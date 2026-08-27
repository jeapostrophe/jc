#!/usr/bin/env bash
# Re-vendor gpui-component from cargo cache and apply local patches.
#
# WARNING: vendor/patches/ does NOT reproduce the vendored tree, so this script
# is lossy. vendor/gpui-component is committed to git and has been edited in
# place and reformatted to this repo's rustfmt.toml (2-space); the patches are
# written against 4-space upstream and will not apply, and some local changes
# (the tree-sitter read-callback and InputEdit crash fix in
# src/highlighter/highlighter.rs and src/input/mode.rs) have no patch file at
# all. After running this, recover local changes from git history --
# `git diff <pre-vendor-rev> -- vendor/gpui-component` -- and re-apply them by
# hand. Do not trust vendor/patches/ to carry them.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
VENDOR_DIR="$ROOT_DIR/vendor/gpui-component"
PATCHES_DIR="$ROOT_DIR/vendor/patches"

if ! git -C "$ROOT_DIR" diff --quiet -- "$VENDOR_DIR" ||
  ! git -C "$ROOT_DIR" diff --cached --quiet -- "$VENDOR_DIR"; then
  echo "ERROR: vendor/gpui-component has uncommitted changes." >&2
  echo "Re-vendoring deletes the tree; these changes are not in vendor/patches/" >&2
  echo "and would be unrecoverable. Commit or stash them first." >&2
  exit 1
fi

echo "WARNING: this overwrites local changes to vendor/gpui-component." >&2
echo "         Recover them afterwards from git history; see the note at the" >&2
echo "         top of this script." >&2

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
