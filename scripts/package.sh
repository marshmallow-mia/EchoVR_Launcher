#!/usr/bin/env bash
# Packages target/release/EchoVR_Launcher into a portable zip (or, windows-setup, the installer).
#   scripts/package.sh <windows|windows-setup|macos|linux> <output.zip|setup.exe>
set -euo pipefail
platform="$1"
out="$(pwd)/$2"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

case "$platform" in
  windows)
    mkdir -p "$stage/EchoVR_Launcher"
    cp target/release/EchoVR_Launcher.exe LICENSE THIRD_PARTY_NOTICES.txt "$stage/EchoVR_Launcher/"
    # The EchoVRCE page's WebView2 loader: a GNU build loads it as a DLL, which must sit next
    # to the exe or the launcher doesn't start.
    loader="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -path '*webview2-com-sys*/x64/WebView2Loader.dll' 2>/dev/null | head -1)"
    if [ -n "$loader" ]; then cp "$loader" "$stage/EchoVR_Launcher/"; fi
    (cd "$stage" && 7z a -tzip "$out" EchoVR_Launcher >/dev/null)
    ;;
  windows-setup)
    # The installer (windows/installer.nsi, NSIS's makensis) of the same files.
    mkdir -p "$stage/EchoVR_Launcher"
    cp target/release/EchoVR_Launcher.exe LICENSE THIRD_PARTY_NOTICES.txt "$stage/EchoVR_Launcher/"
    loader="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -path '*webview2-com-sys*/x64/WebView2Loader.dll' 2>/dev/null | head -1)"
    cp "$loader" "$stage/EchoVR_Launcher/"
    version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
    # Windows' file version is numbers only: 0.11.10 for 0.11.10-beta.1.
    (cd windows && makensis -V2 -DVERSION="$version" -DVIVERSION="${version%%-*}" -DSTAGE="$stage/EchoVR_Launcher" -DOUT="$out" installer.nsi)
    ;;
  macos)
    app="$stage/EchoVR_Launcher.app/Contents"
    mkdir -p "$app/MacOS" "$app/Resources"
    cp target/release/EchoVR_Launcher "$app/MacOS/"
    cp LICENSE THIRD_PARTY_NOTICES.txt "$app/Resources/"
    version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
    cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Echo VR Launcher</string>
  <key>CFBundleIdentifier</key><string>de.echovr.launcher</string>
  <key>CFBundleExecutable</key><string>EchoVR_Launcher</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    (cd "$stage" && zip -qry "$out" EchoVR_Launcher.app)
    ;;
  linux)
    mkdir -p "$stage/EchoVR_Launcher"
    cp target/release/EchoVR_Launcher LICENSE THIRD_PARTY_NOTICES.txt "$stage/EchoVR_Launcher/"
    (cd "$stage" && zip -qr "$out" EchoVR_Launcher)
    ;;
  *)
    echo "unknown platform $platform" >&2
    exit 1
    ;;
esac
ls -lh "$out"
