#!/usr/bin/env bash
set -euo pipefail

SCRIPT="${BASH_SOURCE[0]}"
# Run as `jcc` (a link of that name to this script) for Cira's client, as
# anything else for jc (Cira PLAN TMUX-07).
case "$(basename "$SCRIPT")" in
  jcc) APP="jcc.app/Contents/MacOS/jcc" ;;
  *) APP="jc.app/Contents/MacOS/jc-app" ;;
esac
while [ -L "$SCRIPT" ]; do
  DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"
  SCRIPT="$(readlink "$SCRIPT")"
  case "$SCRIPT" in /*) ;; *) SCRIPT="$DIR/$SCRIPT" ;; esac
done
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"

ARGS=()
for arg in "$@"; do
  ARGS+=("$arg")
done

# Build and bundle (both apps)
"$SCRIPT_DIR/scripts/bundle.sh"

# Run the bundled binary, forwarding all remaining arguments
exec "$SCRIPT_DIR/target/release/$APP" "$@"
