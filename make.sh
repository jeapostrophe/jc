#!/usr/bin/env bash
set -euo pipefail

SCRIPT="${BASH_SOURCE[0]}"
while [ -L "$SCRIPT" ]; do
  DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"
  SCRIPT="$(readlink "$SCRIPT")"
  case "$SCRIPT" in /*) ;; *) SCRIPT="$DIR/$SCRIPT" ;; esac
done
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"

# `--backup-todos` snapshots every registered project's TODO.md before launching.
# Consumed here, not forwarded to the binary. See scripts/backup-todos.sh.
BACKUP_TODOS=0
ARGS=()
for arg in "$@"; do
  case "$arg" in
    --backup-todos) BACKUP_TODOS=1 ;;
    *) ARGS+=("$arg") ;;
  esac
done

# Build and bundle
"$SCRIPT_DIR/scripts/bundle.sh"

# Taken after the build, so the window between the snapshot and the launch that
# truncates is as small as possible -- the currently running jc can still be
# writing TODO.md while the build runs.
if [ "$BACKUP_TODOS" -eq 1 ]; then
  "$SCRIPT_DIR/scripts/backup-todos.sh"
fi

# Run the bundled binary, forwarding all remaining arguments
exec "$SCRIPT_DIR/target/release/jc.app/Contents/MacOS/jc-app" ${ARGS[@]+"${ARGS[@]}"}
