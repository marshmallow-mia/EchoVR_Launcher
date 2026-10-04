#!/usr/bin/env bash
# Refreshes the bundled adb binaries in assets/platform-tools from Google's official
# platform-tools release. Only adb (and the DLLs it needs on Windows) is kept.
#   scripts/fetch-platform-tools.sh 37.0.1
set -euo pipefail
ver="${1:?usage: $0 <platform-tools version, e.g. 37.0.1>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fetch() { # <google os name> <assets dir> <files...>
  local os="$1" dest="$root/assets/platform-tools/$2"; shift 2
  curl -fsSL -o "$tmp/$os.zip" "https://dl.google.com/android/repository/platform-tools_r${ver}-${os}.zip"
  rm -rf "$tmp/platform-tools" && unzip -q "$tmp/$os.zip" -d "$tmp"
  rm -rf "$dest" && mkdir -p "$dest"
  for f in "$@" NOTICE.txt source.properties; do cp "$tmp/platform-tools/$f" "$dest/"; done
}

fetch win windows adb.exe AdbWinApi.dll AdbWinUsbApi.dll libwinpthread-1.dll
fetch darwin macos adb
fetch linux linux adb
echo "Now update VERSION in src/core/adb/bundle.rs to $ver."
