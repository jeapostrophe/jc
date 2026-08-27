#!/usr/bin/env bash
set -euo pipefail

SCRIPT="${BASH_SOURCE[0]}"
while [ -L "$SCRIPT" ]; do
  DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"
  SCRIPT="$(readlink "$SCRIPT")"
  case "$SCRIPT" in /*) ;; *) SCRIPT="$DIR/$SCRIPT" ;; esac
done
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT")" && pwd)"

# ---------------------------------------------------------------------------
# TEMPORARY -- delete this block, the BACKUP_TODOS check further down, and
# scripts/backup-todos.sh once the first post-truncation launch has happened.
#
# jc bounds each session's message log and sweeps every session at startup. The
# first launch after that landed removes a lot of text at once and cannot be
# undone, and jc is a live-in tool -- weeks can pass between restarts, which is
# long enough to forget. So every launch must SAY which it wants; there is no
# default, because the failure mode of the default is silent and permanent.
# ---------------------------------------------------------------------------
BACKUP_TODOS=""
CONFLICT=0
ARGS=()
for arg in "$@"; do
  case "$arg" in
    --backup-todos)
      if [ "$BACKUP_TODOS" = "no" ]; then CONFLICT=1; fi
      BACKUP_TODOS=yes
      ;;
    --no-backup-todos)
      if [ "$BACKUP_TODOS" = "yes" ]; then CONFLICT=1; fi
      BACKUP_TODOS=no
      ;;
    *) ARGS+=("$arg") ;;
  esac
done

if [ "$CONFLICT" -eq 1 ]; then
  echo "make.sh: --backup-todos and --no-backup-todos are mutually exclusive." >&2
  exit 2
fi

# Checked before the build, so a forgotten flag costs a second rather than a
# full compile.
if [ -z "$BACKUP_TODOS" ]; then
  cat >&2 <<'USAGE'
make.sh: refusing to launch without a TODO.md backup decision.

jc now bounds each session's message log to the 25 most recent entries and
sweeps every session at startup. The first launch after that landed drops a
large amount of text at once, and it cannot be undone.

  ./make.sh --backup-todos      snapshot every registered project's TODO.md
                                first -- use this on the first launch
  ./make.sh --no-backup-todos   skip it -- for launches after that snapshot
                                has been taken

Passing --backup-todos again later is harmless: it never overwrites an
existing TODO.md.bak.

This requirement is TEMPORARY. Once the snapshots exist, delete the marked
block in make.sh and scripts/backup-todos.sh.
USAGE
  exit 2
fi

# Build and bundle
"$SCRIPT_DIR/scripts/bundle.sh"

# Taken after the build, so the window between the snapshot and the launch that
# truncates is as small as possible -- the currently running jc can still be
# writing TODO.md while the build runs.
if [ "$BACKUP_TODOS" = "yes" ]; then
  "$SCRIPT_DIR/scripts/backup-todos.sh"
fi

# Run the bundled binary, forwarding all remaining arguments
exec "$SCRIPT_DIR/target/release/jc.app/Contents/MacOS/jc-app" ${ARGS[@]+"${ARGS[@]}"}
