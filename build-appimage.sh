#!/usr/bin/env bash
# Build visual-diff into an AppImage.
#
# Usage:
#   ./build-appimage.sh
#
# Environment variables (optional):
#   FFMPEG_BIN=/path/to/ffmpeg    bundle a static ffmpeg into the AppImage
#   FFPROBE_BIN=/path/to/ffprobe  bundle a static ffprobe too
#                                 (then video support needs no system ffmpeg)
#   ARCH=x86_64|aarch64           target architecture (default: current)
#
# Requires: cargo (Rust), curl or wget.
# Tip: for maximum compatibility build on an older distribution
# (e.g. Ubuntu 22.04), because an AppImage depends on the glibc version.

set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"

APP_NAME="visual-diff"
VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
ARCH="${ARCH:-$(uname -m)}"

BUILD_DIR="build"
APPDIR="$BUILD_DIR/$APP_NAME.AppDir"
DIST_DIR="dist"
TOOL="$BUILD_DIR/appimagetool-$ARCH.AppImage"
TOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
OUTPUT="$DIST_DIR/$APP_NAME-$VERSION-$ARCH.AppImage"

echo "==> Building $APP_NAME $VERSION ($ARCH)"

# 1. Compile the release build
command -v cargo >/dev/null || { echo "Error: cargo not found. Install Rust: https://rustup.rs"; exit 1; }
cargo build --release

# 2. Prepare the AppDir
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$DIST_DIR" "$BUILD_DIR"

install -m 755 "target/release/$APP_NAME" "$APPDIR/usr/bin/$APP_NAME"
strip "$APPDIR/usr/bin/$APP_NAME" 2>/dev/null || true

if [[ -n "${FFMPEG_BIN:-}" ]]; then
    echo "==> Bundling ffmpeg from $FFMPEG_BIN"
    install -m 755 "$FFMPEG_BIN" "$APPDIR/usr/bin/ffmpeg"
fi

if [[ -n "${FFPROBE_BIN:-}" ]]; then
    echo "==> Bundling ffprobe from $FFPROBE_BIN"
    install -m 755 "$FFPROBE_BIN" "$APPDIR/usr/bin/ffprobe"
fi

# 3. AppRun: entry point (puts bundled binaries first in PATH)
cat > "$APPDIR/AppRun" <<EOF
#!/bin/sh
HERE="\$(dirname "\$(readlink -f "\$0")")"
export PATH="\$HERE/usr/bin:\$PATH"
exec "\$HERE/usr/bin/$APP_NAME" "\$@"
EOF
chmod +x "$APPDIR/AppRun"

# 4. .desktop file (GUI application, no terminal needed)
cat > "$APPDIR/$APP_NAME.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Visual Diff
Comment=Compare images and videos: zoom, wipe, PSNR/SSIM/VMAF
Exec=$APP_NAME
Icon=$APP_NAME
Categories=Graphics;Utility;
Terminal=false
StartupWMClass=visual-diff
EOF

# 5. Icon (simple SVG placeholder: two squares and an arrow between them)
cat > "$APPDIR/$APP_NAME.svg" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <rect width="256" height="256" rx="36" fill="#1f2430"/>
  <rect x="28" y="64" width="84" height="128" rx="8" fill="#4f9cf9"/>
  <rect x="144" y="64" width="84" height="128" rx="8" fill="#f9a64f"/>
  <path d="M112 128h32M134 114l14 14-14 14" stroke="#ffffff" stroke-width="8"
        fill="none" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
EOF
ln -sf "$APP_NAME.svg" "$APPDIR/.DirIcon"

# 6. Download appimagetool if missing
if [[ ! -x "$TOOL" ]]; then
    echo "==> Downloading appimagetool"
    if command -v curl >/dev/null; then
        curl -L --fail -o "$TOOL" "$TOOL_URL"
    elif command -v wget >/dev/null; then
        wget -O "$TOOL" "$TOOL_URL"
    else
        echo "Error: curl or wget is required"; exit 1
    fi
    chmod +x "$TOOL"
fi

# 7. Package. APPIMAGE_EXTRACT_AND_RUN lets this work without FUSE (Docker, CI).
echo "==> Packaging the AppImage"
ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" "$APPDIR" "$OUTPUT"

echo
echo "Done: $OUTPUT"
echo "Run the GUI:      $OUTPUT [a.png b.png [original]]"
echo "Console mode:     $OUTPUT --cli a.png b.png result.png"
echo "Choose language:  $OUTPUT --lang en|ru|uk"
