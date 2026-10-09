#!/usr/bin/env bash
# Build a universal Audemo.app, then audemo-macos-universal.zip and Audemo.dmg.
# Run on macOS from the repository root. Needs both Rust targets installed:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
set -euo pipefail

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d '"' -f2)
OUT=dist
APP="$OUT/Audemo.app"

cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

rm -rf "$APP" "$OUT/dmg" "$OUT/Audemo.iconset"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/audemo" \
  target/aarch64-apple-darwin/release/audemo \
  target/x86_64-apple-darwin/release/audemo
sed "s/__VERSION__/$VERSION/g" assets/macos/Info.plist > "$APP/Contents/Info.plist"

# App icon (.icns) from the 1024 px master.
ICONSET="$OUT/Audemo.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icons/audemo-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) assets/icons/audemo-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Audemo.icns"

# Ad-hoc signature so Apple silicon will launch it (not notarised).
codesign --force --deep --sign - "$APP"

# Zip of the bare app.
(cd "$OUT" && rm -f audemo-macos-universal.zip && ditto -c -k --keepParent Audemo.app audemo-macos-universal.zip)

# Drag-to-Applications disk image.
mkdir -p "$OUT/dmg"
cp -R "$APP" "$OUT/dmg/"
ln -s /Applications "$OUT/dmg/Applications"
rm -f "$OUT/Audemo.dmg"
hdiutil create -volname "Audemo $VERSION" -srcfolder "$OUT/dmg" -ov -format UDZO "$OUT/Audemo.dmg"

ls -la "$OUT"
