#!/usr/bin/env python3
"""Log uploads from the launcher (Settings -> Upload logs), kept only when they are logs.

Anyone can send to it, so every upload is untrusted. One upload is one request: a plain
UTF-8 text bundle the launcher builds (src/core/logs.rs), no archive, nothing to unpack:

    ECHOVR-LOGS 1
    FILE <kind>/<name> <bytes>
    <exactly that many bytes>
    ...
    END

It is checked while it streams in, and kept only when every check passed:

1. Before the body: 10 attempts per hour per IP (IPv6 per /64), the path, the content
   type, a Content-Length within the limit, the launcher's user agent, at most two uploads
   at once, room on disk.
2. While it streams: the framing (known kinds, plain names, exact sizes, at most 24 files
   of at most 8 MiB), strict UTF-8, no control, format or private-use characters (escape
   sequences, bidi overrides, zero-width characters...), no carriage return but before a
   line feed, lines of at most 16 KiB, no long base64 runs. Every file also streams to
   ClamAV (clamd) as it arrives. The first violation ends the upload.
3. Each finished file: ClamAV's verdict, then Magika (Google's file type detection) on the
   whole file and on every 2 KiB segment of it (Magika only reads the first and last
   1 KiB of what it is given, so this way it sees every byte). Code, a script or markup
   it is confident of, anywhere, refuses the file. (Its confidence thresholds are its own:
   on real logs the model's raw guesses are noise -- a log segment can look 79% like
   SQL -- while scripts score 0.98 and more.)
4. Kept: the quarantine folder is renamed to its reference code and the code is the
   answer. Anything else: the quarantine is deleted and nothing is kept.

Replies are fixed texts: nothing the client sent is ever echoed back. Client file names
are checked, but the stored files are named by the service. No IP is stored; it is held
in memory for the rate limit only. Uploads are deleted after 30 days. See PRIVACY.md.

Needs Python 3.9+, `magika` (pip, in a venv) and clamd (Debian: clamav-daemon).

  python log_upload.py                   serve (127.0.0.1:8787, behind Apache)
  python log_upload.py --list            the uploads kept
  python log_upload.py --show CODE       print one (its text already passed every check)
  python log_upload.py --delete CODE     delete one (on request)
  python log_upload.py --check FILE...   run every check on files, to tune before going live
"""

from __future__ import annotations

import argparse
import codecs
import hashlib
import ipaddress
import json
import logging
import os
import re
import secrets
import shutil
import socket
import struct
import sys
import threading
import time
import unicodedata
from collections import deque
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Callable, Iterable, Optional

PATH = "/launcher/logs"
STORE = Path(os.environ.get("LOG_UPLOAD_DIR", "/var/lib/echo-launcher-logs"))
CLAMD_SOCKET = os.environ.get("CLAMD_SOCKET", "/var/run/clamav/clamd.ctl")

MAX_BODY = 34 << 20
MAX_FILE = 8 << 20
MAX_FILES = 24
MAX_LINE = 16 << 10
MAX_HEADER_LINE = 256
BASE64_RUN = 1024
RATE_LIMIT = 10
RATE_WINDOW_S = 3600
CONCURRENT = 2
STORAGE_CAP = 2 << 30
RETENTION_S = 30 * 86400
IDLE_TIMEOUT_S = 20
TOTAL_TIMEOUT_S = 180
CHUNK = 64 << 10
# Magika reads the first and last 1 KiB of what it's given: the file, then every segment
# of it at most this long (cut at a line end when there is one), so it sees every byte.
SEGMENT = 2 << 10
# Magika's "code" labels that are only data, which a log may well look like.
DATA = {"csv", "tsv", "json", "jsonl", "ini", "toml", "yaml"}

KINDS = {"launcher", "echo", "echoxr", "quest", "plugin"}
NAME = re.compile(r"[A-Za-z0-9._-]{1,100}")
FILE_LINE = re.compile(rb"FILE ([a-z]{1,16})/([A-Za-z0-9._-]{1,100}) ([0-9]{1,9})")
USER_AGENT = re.compile(r"EchoVR-Installer/[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,6}([-+][0-9A-Za-z.-]{1,32})?")
CODE = re.compile(r"[2-9A-HJ-NP-Z]{8}")
CODE_ALPHABET = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ"
BASE64 = re.compile(rb"[A-Za-z0-9+/=]{%d,}" % BASE64_RUN)

log = logging.getLogger("log_upload")


# ---- what is refused, and why (fixed texts only) ----

REASONS = {
    "framing": "That isn't a log bundle from the launcher.",
    "kind": "A file in it isn't one of the logs the launcher sends.",
    "name": "A file in it has a name the service doesn't take.",
    "files": "There are too many files in it.",
    "file_size": "A file in it is larger than a log the launcher sends.",
    "utf8": "A file in it isn't UTF-8 text.",
    "chars": "A file in it has control or invisible characters, which logs from the launcher never have.",
    "line": "A file in it has a line longer than a log line.",
    "base64": "A file in it carries a long encoded blob, which logs from the launcher never do.",
    "malware": "A file in it was flagged by the virus scanner.",
    "not_text": "A file in it doesn't look like a log.",
    "scanner": "The virus scanner isn't available right now: try again later.",
    "timeout": "The upload took too long.",
}


class Rejected(Exception):
    """An upload refused for `reason` (a key of REASONS)."""

    def __init__(self, reason: str, status: int = 422, detail: str = ""):
        super().__init__(reason)
        self.reason = reason
        self.status = status
        # For the service's own log only, never for the reply.
        self.detail = detail


def _forbidden_class() -> re.Pattern:
    """Every character a log may not have: Unicode categories Cc (but TAB), Cf, Co, Cs, Zl
    and Zp -- escape and other control sequences, bidi overrides (Trojan Source),
    zero-width and tag characters, private use, line and paragraph separators."""
    ranges: list[tuple[int, int]] = []
    for cp in range(0x110000):
        if cp == 0x09:
            continue
        if unicodedata.category(chr(cp)) in ("Cc", "Cf", "Co", "Cs", "Zl", "Zp"):
            if ranges and ranges[-1][1] == cp - 1:
                ranges[-1] = (ranges[-1][0], cp)
            else:
                ranges.append((cp, cp))

    def esc(cp: int) -> str:
        return "\\U%08x" % cp

    body = "".join(esc(a) if a == b else f"{esc(a)}-{esc(b)}" for a, b in ranges)
    return re.compile(f"[{body}]")


FORBIDDEN = _forbidden_class()


# ---- the text checks, while it streams ----

class TextCheck:
    """One file's text, checked line by line as its bytes arrive."""

    def __init__(self) -> None:
        self.rest = b""

    def feed(self, data: bytes) -> None:
        lines = (self.rest + data).split(b"\n")
        self.rest = lines.pop()
        for line in lines:
            check_line(line)
        if len(self.rest) > MAX_LINE:
            raise Rejected("line")

    def finish(self) -> None:
        if self.rest:
            check_line(self.rest)
        self.rest = b""


def check_line(line: bytes) -> None:
    """One line (without its line feed): its length, UTF-8, characters, base64 runs."""
    if len(line) > MAX_LINE:
        raise Rejected("line")
    if line.endswith(b"\r"):
        line = line[:-1]
    try:
        text = line.decode("utf-8", "strict")
    except UnicodeDecodeError:
        raise Rejected("utf8") from None
    m = FORBIDDEN.search(text)
    if m:
        raise Rejected("chars", detail=f"U+{ord(m.group()):04X}")
    if BASE64.search(line):
        raise Rejected("base64")


# ---- ClamAV ----

class Clamd:
    """One file streamed to clamd (INSTREAM) while it arrives."""

    def __init__(self, path: str = CLAMD_SOCKET, timeout: float = 30):
        try:
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.settimeout(timeout)
            self.sock.connect(path)
            self.sock.sendall(b"zINSTREAM\0")
        except OSError as e:
            raise Rejected("scanner", 503, detail=str(e)) from None

    def feed(self, data: bytes) -> None:
        try:
            for i in range(0, len(data), CHUNK):
                part = data[i:i + CHUNK]
                self.sock.sendall(struct.pack(">I", len(part)) + part)
        except OSError as e:
            self.close()
            raise Rejected("scanner", 503, detail=str(e)) from None

    def verdict(self) -> None:
        """Raises unless clamd found nothing."""
        try:
            self.sock.sendall(b"\0\0\0\0")
            reply = b""
            while not reply.endswith(b"\0"):
                part = self.sock.recv(4096)
                if not part:
                    break
                reply += part
        except OSError as e:
            raise Rejected("scanner", 503, detail=str(e)) from None
        finally:
            self.close()
        reply = reply.rstrip(b"\0").decode("ascii", "replace")
        if reply == "stream: OK":
            return
        if reply.endswith("FOUND"):
            raise Rejected("malware", detail=reply)
        raise Rejected("scanner", 503, detail=reply)

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


class NoClamd:
    """Development only (--no-clamd): no virus scan."""

    def feed(self, data: bytes) -> None:
        pass

    def verdict(self) -> None:
        pass

    def close(self) -> None:
        pass


# ---- Magika ----

def magika_code(magika) -> Callable[[bytes], Optional[str]]:
    """What Magika makes of some bytes: the code (script, markup...) it is confident they
    are, or None. Its reported label, after its own per-label confidence thresholds."""

    def code(data: bytes) -> Optional[str]:
        res = magika.identify_bytes(data)
        if not res.ok:
            return "error"
        out = res.prediction.output
        label, group = str(out.label), str(out.group)
        return label if group == "code" and label not in DATA else None

    return code


def segments(data: bytes, size: int = SEGMENT) -> Iterable[bytes]:
    """`data` in pieces of at most `size` bytes, ending at a line end where there is one."""
    start = 0
    while start < len(data):
        end = min(len(data), start + size)
        if end < len(data):
            nl = data.rfind(b"\n", start, end)
            if nl > start:
                end = nl + 1
        yield data[start:end]
        start = end


def check_kind(path: Path, code_of: Callable[[bytes], Optional[str]]) -> None:
    """Magika on a finished file: as a whole, and every segment of it."""
    data = path.read_bytes()
    for part in [data, *segments(data)]:
        code = code_of(part)
        if code:
            raise Rejected("not_text", detail=code)


# ---- the bundle, as it streams ----

class Body:
    """The request body, read in pieces within its Content-Length and the time limit."""

    def __init__(self, rfile, length: int, deadline: float):
        self.rfile = rfile
        self.left = length
        self.deadline = deadline
        self.buf = b""

    def _more(self) -> bytes:
        if time.monotonic() > self.deadline:
            raise Rejected("timeout", 408)
        if self.left <= 0:
            return b""
        try:
            data = self.rfile.read1(min(CHUNK, self.left)) if hasattr(self.rfile, "read1") else self.rfile.read(min(CHUNK, self.left))
        except (socket.timeout, OSError):
            raise Rejected("timeout", 408) from None
        if not data:
            raise Rejected("framing")
        self.left -= len(data)
        return data

    def line(self) -> bytes:
        """The next line (without its line feed), at most MAX_HEADER_LINE bytes."""
        while b"\n" not in self.buf:
            if len(self.buf) > MAX_HEADER_LINE:
                raise Rejected("framing")
            more = self._more()
            if not more:
                raise Rejected("framing")
            self.buf += more
        line, self.buf = self.buf.split(b"\n", 1)
        if len(line) > MAX_HEADER_LINE:
            raise Rejected("framing")
        return line

    def exactly(self, n: int) -> Iterable[bytes]:
        """The next `n` bytes, in pieces."""
        while n > 0:
            if not self.buf:
                self.buf = self._more()
                if not self.buf:
                    raise Rejected("framing")
            part, self.buf = self.buf[:n], self.buf[n:]
            n -= len(part)
            yield part

    def at_end(self) -> bool:
        return not self.buf and self.left == 0

    def drain(self) -> None:
        """Reads what's left and drops it (so a refusal reaches the client instead of a
        reset connection); stops at the time limit."""
        self.buf = b""
        try:
            while self.left > 0 and self._more():
                pass
        except Rejected:
            pass


def receive(body: Body, quarantine: Path, scanner: Callable[[], object],
            labels: Optional[Callable[[bytes], Optional[str]]]) -> list[dict]:
    """Reads, checks and quarantines the bundle; its files, or Rejected."""
    if body.line() != b"ECHOVR-LOGS 1":
        raise Rejected("framing")
    files: list[dict] = []
    while True:
        line = body.line()
        if line == b"END":
            break
        m = FILE_LINE.fullmatch(line)
        if not m:
            raise Rejected("framing")
        kind, name, size = m.group(1).decode(), m.group(2).decode(), int(m.group(3))
        if kind not in KINDS:
            raise Rejected("kind")
        if not NAME.fullmatch(name) or set(name) == {"."}:
            raise Rejected("name")
        if len(files) == MAX_FILES:
            raise Rejected("files")
        if size > MAX_FILE:
            raise Rejected("file_size")
        index = len(files) + 1
        stored = quarantine / f"{index:02d}"
        check, digest, scan = TextCheck(), hashlib.sha256(), scanner()
        fd = os.open(stored, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        try:
            with os.fdopen(fd, "wb") as out:
                for part in body.exactly(size):
                    check.feed(part)
                    scan.feed(part)
                    digest.update(part)
                    out.write(part)
            check.finish()
            scan.verdict()
        finally:
            scan.close()
        if labels is not None:
            check_kind(stored, labels)
        files.append({"kind": kind, "name": name, "bytes": size, "sha256": digest.hexdigest()})
    if not files or not body.at_end():
        raise Rejected("framing")
    return files


# ---- keeping uploads ----

def new_code(store: Path) -> str:
    while True:
        code = "".join(secrets.choice(CODE_ALPHABET) for _ in range(8))
        if not (store / code).exists():
            return code


def stored_name(index: int, f: dict) -> str:
    name = f["name"] if f["name"].lower().endswith((".log", ".txt")) else f["name"] + ".log"
    return f"{index:02d}-{f['kind']}-{name}"


def keep(store: Path, quarantine: Path, files: list[dict], client: str) -> str:
    """Moves a checked upload out of quarantine, under a new code."""
    for i, f in enumerate(files, 1):
        os.rename(quarantine / f"{i:02d}", quarantine / stored_name(i, f))
    meta = {
        "time": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "client": client,
        "files": [{**f, "stored": stored_name(i, f)} for i, f in enumerate(files, 1)],
    }
    fd = os.open(quarantine / "meta.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as out:
        json.dump(meta, out, indent=1)
    code = new_code(store)
    os.rename(quarantine, store / code)
    return code


def used_bytes(store: Path) -> int:
    total = 0
    for dirpath, _, names in os.walk(store):
        for n in names:
            try:
                total += os.lstat(os.path.join(dirpath, n)).st_size
            except OSError:
                pass
    return total


def clean_up(store: Path, now: float) -> None:
    """Deletes uploads past their 30 days, and quarantines an upload left behind."""
    for entry in store.iterdir():
        try:
            age = now - entry.stat().st_mtime
        except OSError:
            continue
        if entry.name == ".incoming":
            for q in entry.iterdir():
                try:
                    if now - q.stat().st_mtime > 3600:
                        shutil.rmtree(q, ignore_errors=True)
                except OSError:
                    pass
        elif CODE.fullmatch(entry.name) and age > RETENTION_S:
            shutil.rmtree(entry, ignore_errors=True)
            log.info("deleted %s (30 days)", entry.name)


# ---- the rate limit ----

class RateLimit:
    """At most `limit` attempts per `window` seconds per IP (IPv6: per /64), in memory."""

    def __init__(self, limit: int = RATE_LIMIT, window: float = RATE_WINDOW_S):
        self.limit, self.window = limit, window
        self.seen: dict[str, deque] = {}
        self.lock = threading.Lock()

    @staticmethod
    def key(ip: str) -> str:
        try:
            addr = ipaddress.ip_address(ip)
        except ValueError:
            return "unknown"
        if isinstance(addr, ipaddress.IPv6Address):
            if addr.ipv4_mapped is not None:
                return str(addr.ipv4_mapped)
            return str(ipaddress.ip_network(f"{addr}/64", strict=False))
        return str(addr)

    def allow(self, ip: str, now: Optional[float] = None) -> bool:
        now = time.monotonic() if now is None else now
        k = self.key(ip)
        with self.lock:
            q = self.seen.setdefault(k, deque())
            while q and now - q[0] > self.window:
                q.popleft()
            q.append(now)
            if len(self.seen) > 100_000:
                for old in [key for key, d in self.seen.items() if not d or now - d[-1] > self.window]:
                    del self.seen[old]
            return len(q) <= self.limit


def client_ip(peer: str, real_ip: Optional[str]) -> str:
    """The client's address: Apache's X-Real-IP when Apache (on this host) asks, else the
    peer itself."""
    if peer in ("127.0.0.1", "::1") and real_ip:
        return real_ip.strip()
    return peer


# ---- the service ----

class Service:
    def __init__(self, store: Path, scanner: Callable[[], object],
                 labels: Optional[Callable[[bytes], Optional[str]]]):
        self.store = store
        self.scanner = scanner
        self.labels = labels
        self.rate = RateLimit()
        self.slots = threading.BoundedSemaphore(CONCURRENT)
        (store / ".incoming").mkdir(parents=True, exist_ok=True, mode=0o700)


def handler(service: Service):
    class Handler(BaseHTTPRequestHandler):
        server_version = "echo-launcher-logs"
        sys_version = ""
        timeout = IDLE_TIMEOUT_S

        def log_message(self, fmt, *args):  # The request line is the client's: never logged.
            pass

        def reply(self, status: int, body: dict) -> None:
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(data)
            self.close_connection = True

        def refuse(self, status: int, reason: str) -> None:
            self.reply(status, {"error": REASONS.get(reason, "The upload was refused.")})

        def early(self, status: int, error: str) -> None:
            """Refuses before reading the body: drops the body first (a sane one only)."""
            length = self.headers.get("Content-Length", "")
            if length.isdigit() and int(length) <= MAX_BODY:
                Body(self.rfile, int(length), time.monotonic() + IDLE_TIMEOUT_S).drain()
            self.reply(status, {"error": error})

        def do_GET(self):
            self.reply(405, {"error": "Only uploads from the launcher."})

        do_PUT = do_DELETE = do_HEAD = do_PATCH = do_OPTIONS = do_GET

        def do_POST(self):
            try:
                self.post()
            except Exception:  # noqa: BLE001 -- fail closed, whatever it was
                log.exception("request failed")
                try:
                    self.reply(500, {"error": "The upload failed."})
                except OSError:
                    pass

        def post(self):
            ip = client_ip(self.client_address[0], self.headers.get("X-Real-IP"))
            if not service.rate.allow(ip):
                log.info("rate limited")
                return self.early(429, "Too many uploads: try again in an hour.")
            if self.path != PATH:
                return self.early(404, "Not here.")
            if self.headers.get("Content-Type", "").replace(" ", "").lower() != "text/plain;charset=utf-8":
                return self.early(415, "Only text.")
            if self.headers.get("Transfer-Encoding") or self.headers.get("Content-Encoding"):
                return self.reply(411, {"error": "A plain body with its length, please."})
            length = self.headers.get("Content-Length", "")
            if not length.isdigit():
                return self.reply(411, {"error": "A plain body with its length, please."})
            if int(length) > MAX_BODY:
                return self.reply(413, {"error": "Larger than the logs the launcher sends."})
            if not USER_AGENT.fullmatch(self.headers.get("User-Agent", "")):
                return self.early(403, "Only uploads from the launcher.")
            if not service.slots.acquire(blocking=False):
                return self.early(503, "Busy: try again in a few minutes.")
            try:
                if used_bytes(service.store) > STORAGE_CAP:
                    log.warning("storage full")
                    return self.early(503, "Busy: try again in a few minutes.")
                self.upload(int(length))
            finally:
                service.slots.release()

        def upload(self, length: int) -> None:
            quarantine = service.store / ".incoming" / secrets.token_hex(8)
            quarantine.mkdir(mode=0o700)
            body = Body(self.rfile, length, time.monotonic() + TOTAL_TIMEOUT_S)
            try:
                files = receive(body, quarantine, service.scanner, service.labels)
                code = keep(service.store, quarantine, files,
                            self.headers.get("User-Agent", ""))
            except Rejected as r:
                # Gone before anyone hears of it.
                shutil.rmtree(quarantine, ignore_errors=True)
                log.info("refused: %s %s", r.reason, r.detail)
                body.drain()
                return self.refuse(r.status, r.reason)
            except Exception:  # noqa: BLE001 -- fail closed, whatever it was
                shutil.rmtree(quarantine, ignore_errors=True)
                log.exception("upload failed")
                return self.reply(500, {"error": "The upload failed."})
            log.info("kept %s: %d file(s), %d bytes", code, len(files), sum(f["bytes"] for f in files))
            self.reply(200, {"code": code})

    return Handler


def serve(service: Service, host: str, port: int) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer((host, port), handler(service))
    server.daemon_threads = True
    return server


def cleaner(store: Path) -> None:
    while True:
        try:
            clean_up(store, time.time())
        except Exception:  # noqa: BLE001
            log.exception("clean up failed")
        time.sleep(3600)


# ---- the command line ----

def show(store: Path, code: str) -> None:
    if not CODE.fullmatch(code):
        sys.exit("not a reference code")
    folder = store / code
    meta = json.loads((folder / "meta.json").read_text())
    print(f"{code}  {meta['time']}  {meta['client']}")
    for f in meta["files"]:
        print(f"\n===== {f['kind']}/{f['name']} ({f['bytes']} bytes, sha256 {f['sha256']}) =====")
        sys.stdout.write((folder / f["stored"]).read_text("utf-8"))


def listing(store: Path) -> None:
    for entry in sorted(store.iterdir()):
        if CODE.fullmatch(entry.name):
            meta = json.loads((entry / "meta.json").read_text())
            size = sum(f["bytes"] for f in meta["files"])
            print(f"{entry.name}  {meta['time']}  {len(meta['files'])} file(s)  {size} bytes  {meta['client']}")


def check_files(paths: list[str], scanner, labels) -> int:
    """Every check on files as they are, as if each came in an upload."""
    bad = 0
    for p in paths:
        data = Path(p).read_bytes()
        try:
            if len(data) > MAX_FILE:
                raise Rejected("file_size")
            check, scan = TextCheck(), scanner()
            try:
                for i in range(0, len(data), CHUNK):
                    check.feed(data[i:i + CHUNK])
                    scan.feed(data[i:i + CHUNK])
                check.finish()
                scan.verdict()
            finally:
                scan.close()
            if labels is not None:
                check_kind(Path(p), labels)
            print(f"ok       {p}")
        except Rejected as r:
            bad += 1
            print(f"refused  {p}: {r.reason} {r.detail}")
    return 1 if bad else 0


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8787)
    ap.add_argument("--dir", type=Path, default=STORE)
    ap.add_argument("--no-clamd", action="store_true", help="development only: no virus scan")
    ap.add_argument("--no-magika", action="store_true", help="development only: no file type check")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--show", metavar="CODE")
    ap.add_argument("--delete", metavar="CODE")
    ap.add_argument("--check", nargs="+", metavar="FILE")
    a = ap.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)-7s %(message)s")

    if a.list:
        return listing(a.dir)
    if a.show:
        return show(a.dir, a.show)
    if a.delete:
        if not CODE.fullmatch(a.delete) or not (a.dir / a.delete).is_dir():
            sys.exit("no such upload")
        shutil.rmtree(a.dir / a.delete)
        return print(f"deleted {a.delete}")

    scanner = NoClamd if a.no_clamd else Clamd
    labels = None
    if not a.no_magika:
        from magika import Magika  # the venv's

        labels = magika_code(Magika())
    if a.check:
        sys.exit(check_files(a.check, scanner, labels))

    a.dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    service = Service(a.dir, scanner, labels)
    threading.Thread(target=cleaner, args=(a.dir,), daemon=True).start()
    server = serve(service, a.host, a.port)
    log.info("listening on %s:%d, keeping uploads in %s", a.host, a.port, a.dir)
    server.serve_forever()


if __name__ == "__main__":
    main()
