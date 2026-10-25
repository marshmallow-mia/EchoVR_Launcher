#!/usr/bin/env python3
"""Signs launcher plugins and the plugins catalogue with the plugins key (Ed25519).

  scripts/sign.py plugin <folder> [--zip out.zip]   writes <folder>/plugin.sig
  scripts/sign.py file <path>                       writes <path>.sig (plugins.v2.json)

The key is ~/echovr-signing-key/plugins.key (PEM, kept offline) unless --key names
another. OpenSSL 3 makes the signatures (`pkeyutl -sign -rawin`); $OPENSSL names it when
the one on PATH is LibreSSL (macOS). The launcher checks them in src/core/signing.rs.
"""
import argparse
import base64
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile

STATEMENT = "echovr-plugin-v1"
SIG = "plugin.sig"


def openssl():
    for o in [os.environ.get("OPENSSL"), "/opt/homebrew/opt/openssl@3/bin/openssl", "openssl"]:
        if not o or not shutil.which(o):
            continue
        v = subprocess.run([o, "version"], capture_output=True, text=True).stdout
        if v.startswith("OpenSSL 3"):
            return o
    sys.exit("sign.py: needs OpenSSL 3 (set OPENSSL)")


def key_id(key):
    der = subprocess.run(
        [openssl(), "pkey", "-in", key, "-pubout", "-outform", "DER"],
        capture_output=True, check=True,
    ).stdout
    return hashlib.sha256(der[-32:]).hexdigest()[:16]


def sign(key, data):
    with tempfile.NamedTemporaryFile() as f:
        f.write(data)
        f.flush()
        sig = subprocess.run(
            [openssl(), "pkeyutl", "-sign", "-rawin", "-inkey", key, "-in", f.name],
            capture_output=True, check=True,
        ).stdout
    if len(sig) != 64:
        sys.exit("sign.py: OpenSSL gave no Ed25519 signature")
    return {
        "key": key_id(key),
        "sha256": hashlib.sha256(data).hexdigest(),
        "sig": base64.b64encode(sig).decode(),
    }


def plugin_files(folder):
    """The plugin's files by path (/ between folders): not dot files or folders, not
    plugin.sig."""
    out = {}
    for root, dirs, files in os.walk(folder):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for name in files:
            if name.startswith("."):
                continue
            rel = os.path.relpath(os.path.join(root, name), folder).replace(os.sep, "/")
            if rel == SIG:
                continue
            if "\n" in rel or "  " in rel:
                sys.exit(f"sign.py: can't sign a file named {rel!r}")
            with open(os.path.join(root, name), "rb") as f:
                out[rel] = hashlib.sha256(f.read()).hexdigest()
    return out


def statement(folder):
    with open(os.path.join(folder, "plugin.json"), encoding="utf-8") as f:
        desc = json.load(f)
    pid, version = desc.get("id", ""), desc.get("version", "")
    if not pid or not version:
        sys.exit("sign.py: plugin.json needs its id and version")
    lines = [STATEMENT, f"id {pid}", f"version {version}"]
    files = plugin_files(folder)
    lines += [f"{sha}  {path}" for path, sha in sorted(files.items())]
    return "\n".join(lines) + "\n", pid, version, files


def cmd_plugin(a):
    text, pid, version, files = statement(a.folder)
    sig = {"schema": 1, "statement": text, "signatures": [sign(a.key, text.encode())]}
    with open(os.path.join(a.folder, SIG), "w", encoding="utf-8") as f:
        json.dump(sig, f, indent=2)
        f.write("\n")
    print(f"signed {pid} {version}: {len(files)} files")
    if a.zip:
        with zipfile.ZipFile(a.zip, "w", zipfile.ZIP_DEFLATED) as z:
            for rel in sorted(list(files) + [SIG]):
                z.write(os.path.join(a.folder, rel), rel)
        with open(a.zip, "rb") as f:
            data = f.read()
        print(f"{a.zip}: {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}")


def cmd_file(a):
    with open(a.path, "rb") as f:
        data = f.read()
    new = sign(a.key, data)
    keep = []
    try:
        with open(a.path + ".sig", encoding="utf-8") as f:
            old = json.load(f).get("signatures", [])
        # The one before stays beside it (a cache may still have that text).
        keep = [s for s in old if s.get("sha256") != new["sha256"]][:1]
    except (OSError, ValueError):
        pass
    with open(a.path + ".sig", "w", encoding="utf-8") as f:
        json.dump({"schema": 1, "signatures": [new] + keep}, f, indent=2)
        f.write("\n")
    print(f"signed {a.path} (sha256 {new['sha256']})")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    p.add_argument("--key", default=os.path.expanduser("~/echovr-signing-key/plugins.key"))
    sub = p.add_subparsers(dest="cmd", required=True)
    pl = sub.add_parser("plugin")
    pl.add_argument("folder")
    pl.add_argument("--zip")
    fi = sub.add_parser("file")
    fi.add_argument("path")
    a = p.parse_args()
    {"plugin": cmd_plugin, "file": cmd_file}[a.cmd](a)


if __name__ == "__main__":
    main()
