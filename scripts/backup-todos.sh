#!/usr/bin/env bash
# Snapshot every registered project's TODO.md to TODO.md.bak.
#
# jc bounds each session's message log to the 25 most recent entries and sweeps
# every session at startup, which on the first launch after that feature landed
# removes a large amount of text at once. The app writes its own TODO.md.bak
# before that sweep; this is the independent copy, taken from the shell just
# before launch, for the one launch where that matters.
#
# Never clobbers an existing backup -- the point is to capture the state before
# anything truncated, so the FIRST snapshot is the one worth keeping.
#
# Exits non-zero if any copy fails: a launch that truncates is not something to
# proceed with when the safety net is missing.
set -euo pipefail

STATE_FILE="$HOME/.config/jc/state.toml"

if [ ! -f "$STATE_FILE" ]; then
  echo "backup-todos: no project registry at $STATE_FILE; nothing to do." >&2
  exit 0
fi

# `[[projects]]` blocks carry a single `path = "..."` line each.
PROJECTS=$(sed -n 's/^path = "\(.*\)"$/\1/p' "$STATE_FILE")

if [ -z "$PROJECTS" ]; then
  echo "backup-todos: no projects registered; nothing to do." >&2
  exit 0
fi

created=0
skipped=0
missing=0

while IFS= read -r project; do
  [ -n "$project" ] || continue
  todo="$project/TODO.md"
  backup="$todo.bak"

  if [ ! -f "$todo" ]; then
    missing=$((missing + 1))
    continue
  fi
  if [ -e "$backup" ]; then
    echo "  kept    $backup (already exists)"
    skipped=$((skipped + 1))
    continue
  fi

  cp -p "$todo" "$backup"
  echo "  backed up $todo -> $backup ($(wc -l <"$todo" | tr -d ' ') lines)"
  created=$((created + 1))
done <<EOF
$PROJECTS
EOF

echo "backup-todos: $created created, $skipped already present, $missing without a TODO.md"
