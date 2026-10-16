#!/usr/bin/env bash
# Builds dist/zlaty-kalich-<version>-x86_64.AppImage. Needs libfuse2 to run linuxdeploy.
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')
cargo build --release

appdir=target/AppDir
rm -rf "$appdir"
install -Dm755 target/release/zlaty-kalich "$appdir/usr/bin/zlaty-kalich"
install -Dm644 assets/zlaty-kalich.desktop "$appdir/usr/share/applications/zlaty-kalich.desktop"
install -Dm644 assets/icon-256.png "$appdir/usr/share/icons/hicolor/256x256/apps/zlaty-kalich.png"

tool=target/linuxdeploy-x86_64.AppImage
if [ ! -x "$tool" ]; then
  curl -fsSL -o "$tool" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage
  chmod +x "$tool"
fi
mkdir -p dist
OUTPUT="dist/zlaty-kalich-${version}-x86_64.AppImage" "$tool" --appdir "$appdir" --output appimage
