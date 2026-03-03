#!/usr/bin/env bash
# Re-vendor gpui-component from cargo cache and apply local patches.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
VENDOR_DIR="$ROOT_DIR/vendor/gpui-component"
PATCHES_DIR="$ROOT_DIR/vendor/patches"

# Locate the cached crate (adjust version as needed).
VERSION="0.5.1"
CACHE_DIR=$(find "$HOME/.cargo/registry/src" -maxdepth 1 -type d -name "index.crates.io-*" | head -1)
SOURCE="$CACHE_DIR/gpui-component-$VERSION"

if [ ! -d "$SOURCE" ]; then
  echo "ERROR: gpui-component $VERSION not found in cargo cache."
  echo "Run 'cargo fetch' first."
  exit 1
fi

echo "Copying gpui-component $VERSION from cargo cache..."
rm -rf "$VENDOR_DIR"
cp -R "$SOURCE" "$VENDOR_DIR"

echo "Applying patches..."
for patch in "$PATCHES_DIR"/gpui-component-*.patch; do
  [ -f "$patch" ] || continue
  echo "  Applying $(basename "$patch")..."
  patch -d "$VENDOR_DIR" -p1 < "$patch"
done

echo "Done. Vendored gpui-component is up to date."
