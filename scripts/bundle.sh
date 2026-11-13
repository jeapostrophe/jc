#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# One binary, two apps (Cira PLAN TMUX-07): jc.app runs it as `jc-app` (jc,
# its own Claude sessions); jcc.app runs the same binary named `jcc`, which
# makes it Cira's client, with its own bundle id (com.jc.app.jcc), config
# directory (~/.config/jcc) and TODO.md section (`# Cira`).

echo "Building jc-app (release)..."
(cd "$PROJECT_ROOT" && cargo build --release -p jc-app)

# Generate AppIcon.icns with proper HIG-compliant transparent padding
# (824x824 content centered on 1024x1024 canvas). Cached outside the bundle and
# regenerated only when icon.png changes: `jc` re-bundles on every launch, and
# ImageMagick (Homebrew, links freetype) must not be a runtime dependency of
# launching the app.
ICON_CACHE="$PROJECT_ROOT/target/release/AppIcon.icns"
if [ ! -f "$ICON_CACHE" ] || [ "$PROJECT_ROOT/icon.png" -nt "$ICON_CACHE" ]; then
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

  cp "$ICON_TMP/icon.icns" "$ICON_CACHE.tmp"
  mv "$ICON_CACHE.tmp" "$ICON_CACHE"
  rm -rf "$ICON_TMP"
fi

# bundle APP EXECUTABLE PLIST: one app bundle around the release binary.
bundle() {
  local app="$PROJECT_ROOT/target/release/$1"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$PROJECT_ROOT/jc-app/$3" "$app/Contents/Info.plist"
  cp "$PROJECT_ROOT/target/release/jc-app" "$app/Contents/MacOS/$2"
  cp "$ICON_CACHE" "$app/Contents/Resources/AppIcon.icns"
  # Ad-hoc codesign
  codesign --force --sign - "$app"
  echo "Bundle created at: $app"
}

bundle jc.app jc-app Info.plist
bundle jcc.app jcc Info-jcc.plist
