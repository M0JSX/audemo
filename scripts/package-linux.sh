#!/usr/bin/env bash
# Build audemo-linux-x64.tar.gz, audemo_<ver>_amd64.deb and Audemo-x86_64.AppImage
# from target/release/audemo. Run from the repository root after `cargo build --release`.
set -euo pipefail

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d '"' -f2)
BIN=target/release/audemo
OUT=dist
mkdir -p "$OUT"

# ---- portable tarball
rm -rf "$OUT/audemo"
mkdir -p "$OUT/audemo"
cp "$BIN" "$OUT/audemo/"
cp assets/linux/audemo.desktop assets/icons/audemo.png README.md "$OUT/audemo/"
tar -C "$OUT" -czf "$OUT/audemo-linux-x64.tar.gz" audemo

# ---- .deb
PKG="$OUT/deb/audemo_${VERSION}_amd64"
rm -rf "$OUT/deb"
install -Dm755 "$BIN" "$PKG/usr/bin/audemo"
install -Dm644 assets/linux/audemo.desktop "$PKG/usr/share/applications/audemo.desktop"
install -Dm644 assets/icons/audemo.png "$PKG/usr/share/icons/hicolor/256x256/apps/audemo.png"
install -Dm644 README.md "$PKG/usr/share/doc/audemo/README.md"
SIZE=$(du -sk "$PKG/usr" | cut -f1)
mkdir -p "$PKG/DEBIAN"
cat > "$PKG/DEBIAN/control" <<EOF
Package: audemo
Version: ${VERSION}
Section: sound
Priority: optional
Architecture: amd64
Installed-Size: ${SIZE}
Depends: libc6 (>= 2.35), libasound2 (>= 1.0.16), libxkbcommon0, libgl1, libx11-6
Recommends: xdg-desktop-portal
Maintainer: Jonathan <jonathan@m0jsx.radio>
Description: Waveform audio editor with a built-in effects suite
 Audemo is a native waveform editor with a spectral display, markers,
 undo history, recording and 39 effects including EQ, dynamics, noise
 reduction, reverb, modulation and time/pitch processing.
EOF
dpkg-deb --build --root-owner-group "$PKG" "$OUT/audemo_${VERSION}_amd64.deb"

# ---- AppImage (skipped if linuxdeploy can't be fetched, e.g. offline)
if [ "${SKIP_APPIMAGE:-0}" != "1" ]; then
  TOOLS="$OUT/tools"
  mkdir -p "$TOOLS"
  LD="$TOOLS/linuxdeploy-x86_64.AppImage"
  if [ ! -x "$LD" ]; then
    curl -fsSL -o "$LD" https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
    chmod +x "$LD"
  fi
  rm -rf "$OUT/AppDir"
  OUTPUT="$OUT/Audemo-x86_64.AppImage" LDAI_OUTPUT="$OUT/Audemo-x86_64.AppImage" \
    "$LD" --appimage-extract-and-run \
      --appdir "$OUT/AppDir" \
      --executable "$BIN" \
      --desktop-file assets/linux/audemo.desktop \
      --icon-file assets/icons/audemo.png \
      --output appimage
  # Older linuxdeploy releases ignore OUTPUT and write to the current directory.
  for f in Audemo*.AppImage; do
    [ -e "$f" ] && mv "$f" "$OUT/Audemo-x86_64.AppImage"
  done
fi

ls -la "$OUT"
