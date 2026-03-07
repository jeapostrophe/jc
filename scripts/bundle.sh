#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_BUNDLE="$PROJECT_ROOT/target/release/jc.app"
CONTENTS_DIR="$APP_BUNDLE/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "Building jc-app (release)..."
cargo build --release -p jc-app

# Clean any previous bundle
if [ -d "$APP_BUNDLE" ]; then
    rm -rf "$APP_BUNDLE"
fi

# Create bundle structure
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# Copy Info.plist
cp "$PROJECT_ROOT/jc-app/Info.plist" "$CONTENTS_DIR/Info.plist"

# Copy binary
cp "$PROJECT_ROOT/target/release/jc-app" "$MACOS_DIR/jc-app"

# Convert icon.png to AppIcon.icns
ICON_SRC="$PROJECT_ROOT/icon.png"
ICONSET_DIR=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET_DIR"

sips -z 16 16     "$ICON_SRC" --out "$ICONSET_DIR/icon_16x16.png"      > /dev/null 2>&1
sips -z 32 32     "$ICON_SRC" --out "$ICONSET_DIR/icon_16x16@2x.png"   > /dev/null 2>&1
sips -z 32 32     "$ICON_SRC" --out "$ICONSET_DIR/icon_32x32.png"      > /dev/null 2>&1
sips -z 64 64     "$ICON_SRC" --out "$ICONSET_DIR/icon_32x32@2x.png"   > /dev/null 2>&1
sips -z 128 128   "$ICON_SRC" --out "$ICONSET_DIR/icon_128x128.png"    > /dev/null 2>&1
sips -z 256 256   "$ICON_SRC" --out "$ICONSET_DIR/icon_128x128@2x.png" > /dev/null 2>&1
sips -z 256 256   "$ICON_SRC" --out "$ICONSET_DIR/icon_256x256.png"    > /dev/null 2>&1
sips -z 512 512   "$ICON_SRC" --out "$ICONSET_DIR/icon_256x256@2x.png" > /dev/null 2>&1
sips -z 512 512   "$ICON_SRC" --out "$ICONSET_DIR/icon_512x512.png"    > /dev/null 2>&1
sips -z 1024 1024 "$ICON_SRC" --out "$ICONSET_DIR/icon_512x512@2x.png" > /dev/null 2>&1

iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/AppIcon.icns"

# Clean up temp iconset
rm -rf "$(dirname "$ICONSET_DIR")"

# Ad-hoc codesign
codesign --force --sign - "$APP_BUNDLE"

echo "Bundle created at: $APP_BUNDLE"
