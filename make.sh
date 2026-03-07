#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Build and bundle
"$SCRIPT_DIR/scripts/bundle.sh"

# Run the bundled binary, forwarding all arguments
exec "$SCRIPT_DIR/target/release/jc.app/Contents/MacOS/jc-app" "$@"
