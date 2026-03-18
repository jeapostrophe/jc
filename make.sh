#!/usr/bin/env bash
set -euo pipefail

SCRIPT="${BASH_SOURCE[0]}"
while [ -L "$SCRIPT" ]; do
  DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"
  SCRIPT="$(readlink "$SCRIPT")"
  case "$SCRIPT" in /*) ;; *) SCRIPT="$DIR/$SCRIPT" ;; esac
done
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"

# Build and bundle
"$SCRIPT_DIR/scripts/bundle.sh"

# Run the bundled binary, forwarding all arguments
exec "$SCRIPT_DIR/target/release/jc.app/Contents/MacOS/jc-app" "$@"
