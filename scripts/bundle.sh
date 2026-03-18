#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_BUNDLE="$PROJECT_ROOT/target/release/jc.app"
CONTENTS_DIR="$APP_BUNDLE/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "Building jc-app (release)..."
(cd "$PROJECT_ROOT" && cargo build --release -p jc-app)

rm -rf "$APP_BUNDLE"

# Create bundle structure
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# Copy Info.plist
cp "$PROJECT_ROOT/jc-app/Info.plist" "$CONTENTS_DIR/Info.plist"

# Copy binary
cp "$PROJECT_ROOT/target/release/jc-app" "$MACOS_DIR/jc-app"

# Generate AppIcon.icns with proper HIG-compliant transparent padding
# (824x824 content centered on 1024x1024 canvas).
ICON_SRC="$PROJECT_ROOT/icon.png"
ICON_TMP=$(mktemp -d)
COMMON_ICONS="$PROJECT_ROOT/../common/icons/gen-icons.sh"

if [ -x "$COMMON_ICONS" ]; then
  "$COMMON_ICONS" "$ICON_SRC" "$ICON_TMP"
else
  # Inline fallback: same logic as gen-icons.sh
  MASTER="$ICON_TMP/master.png"
  ICONSET="$ICON_TMP/AppIcon.iconset"
  mkdir -p "$ICONSET"
  magick "$ICON_SRC" -resize 824x824 -gravity center -background none -extent 1024x1024 "$MASTER"
  for spec in \
    "1024 icon_512x512@2x" "512 icon_512x512" "512 icon_256x256@2x" \
    "256 icon_256x256" "256 icon_128x128@2x" "128 icon_128x128" \
    "64 icon_32x32@2x" "32 icon_32x32" "32 icon_16x16@2x" "16 icon_16x16"; do
    size="${spec%% *}"; name="${spec##* }"
    magick "$MASTER" -resize "${size}x${size}" "PNG32:$ICONSET/${name}.png"
  done
  iconutil -c icns "$ICONSET" --output "$ICON_TMP/icon.icns"
  rm "$MASTER"
fi

cp "$ICON_TMP/icon.icns" "$RESOURCES_DIR/AppIcon.icns"
rm -rf "$ICON_TMP"

# Ad-hoc codesign
codesign --force --sign - "$APP_BUNDLE"

echo "Bundle created at: $APP_BUNDLE"
