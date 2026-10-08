#!/usr/bin/env bash
# Packages target/release/EchoVR_Launcher into a portable zip (or, windows-setup, the installer;
# appimage, the Linux AppImage, made on Linux with a pinned appimagetool and AppImage runtime).
#   scripts/package.sh <windows|windows-setup|macos|linux|appimage> <output.zip|setup.exe|.AppImage>
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
  appimage)
    # One file that runs on its own (x86_64; glibc 2.31 or newer, as the build's). The
    # launcher updates it in place (core::launcher::self_update).
    app="$stage/EchoVR_Launcher.AppDir"
    mkdir -p "$app/usr/bin" "$app/usr/share/doc/echovr-launcher"
    cp target/release/EchoVR_Launcher "$app/usr/bin/"
    cp LICENSE THIRD_PARTY_NOTICES.txt "$app/usr/share/doc/echovr-launcher/"
    cp linux/de.echovr.launcher.desktop "$app/"
    cp assets/img/icon.png "$app/de.echovr.launcher.png"
    ln -s usr/bin/EchoVR_Launcher "$app/AppRun"
    # appimagetool 1.9.1 and the AppImage runtime of 2025-11-08, each checked by its SHA-256.
    tools="${XDG_CACHE_HOME:-$HOME/.cache}/echovr-launcher-build"
    mkdir -p "$tools"
    fetch() { # fetch <file> <url> <sha256>
      [ -f "$tools/$1" ] || curl -fsSLo "$tools/$1" "$2"
      echo "$3  $tools/$1" | sha256sum -c --quiet
      chmod +x "$tools/$1"
    }
    fetch appimagetool-1.9.1-x86_64.AppImage \
      https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
      ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    fetch runtime-20251108-x86_64 \
      https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
      2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
    ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tools/appimagetool-1.9.1-x86_64.AppImage" \
      --no-appstream --runtime-file "$tools/runtime-20251108-x86_64" "$app" "$out" >"$stage/appimagetool.log" 2>&1 \
      || { cat "$stage/appimagetool.log" >&2; exit 1; }
    ;;
  *)
    echo "unknown platform $platform" >&2
    exit 1
    ;;
esac
ls -lh "$out"
