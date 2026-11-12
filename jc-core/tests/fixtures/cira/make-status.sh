#!/bin/sh
# Regenerates status.json, the `cira status --json` fixture of jc_core::cira's tests, from a
# real Cira daemon: a scratch store (never ~/.cira), its own port, hook calls that put one
# session in each state, and two posted blocks.
#
#   make-status.sh /path/to/cira
#
# Each session's CIRA_SESSION is `fx-<state>`, so a test can read the state a session was
# driven to from its id. Projects `outer` and `outer/inner` nest, for the deepest-path match.
set -eu
# This shell may itself run inside a Claude Code session: none of its identity may leak in.
unset CLAUDE_CODE_SESSION_ID CLAUDE_PID CLAUDE_JOB_DIR TMUX_PANE CLAUDE_CODE_MESSAGING_SOCKET CIRA_SESSION
CIRA=${1:?usage: make-status.sh /path/to/cira}
HERE=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d /tmp/jcfx.XXXXXX)
export CIRA_HOME="$T/home" CIRA_NO_LIVE_HOME=1
mkdir -p "$CIRA_HOME" "$T/outer/inner"
OUTER=$(cd "$T/outer" && pwd -P)
INNER="$OUTER/inner"
"$CIRA" daemon --port 0 > "$T/daemon.out" 2>&1 &
DPID=$!
trap 'kill $DPID 2>/dev/null; wait $DPID 2>/dev/null; rm -rf "$T"' EXIT
i=0; until "$CIRA" ping >/dev/null 2>&1; do i=$((i+1)); [ $i -lt 100 ] || { cat "$T/daemon.out"; exit 1; }; sleep 0.1; done
"$CIRA" project add outer --path "$OUTER" >/dev/null
"$CIRA" project add inner --path "$INNER" >/dev/null

# hook EVENT SESSION SHORT CWD [EXTRA-JSON-FIELDS]
hook() {
  printf '{"session_id":"%s-0000-4000-8000-000000000000","cwd":"%s"%s}' "$3" "$4" "${5:-}" |
    CIRA_SESSION="$2" CLAUDE_JOB_DIR="/x/jobs/$3" "$CIRA" hook "$1" >/dev/null
}
start() { hook session-start "$1" "$2" "$3" ',"source":"startup"'; }

start fx-working aaaa0001 "$OUTER"; hook user-prompt-submit fx-working aaaa0001 "$OUTER" ',"prompt":"go"'
start fx-idle aaaa0002 "$OUTER"; hook user-prompt-submit fx-idle aaaa0002 "$OUTER" ',"prompt":"go"'
hook stop fx-idle aaaa0002 "$OUTER"
start fx-waiting aaaa0003 "$OUTER"; hook user-prompt-submit fx-waiting aaaa0003 "$OUTER" ',"prompt":"go"'
hook stop fx-waiting aaaa0003 "$OUTER" ',"background_tasks":[{"id":"t1"}]'
start fx-dialog aaaa0004 "$INNER"; hook permission-request fx-dialog aaaa0004 "$INNER"
start fx-gone aaaa0005 "$INNER"; hook session-end fx-gone aaaa0005 "$INNER" ',"reason":"logout"'

post() {
  (cd "$1" && CIRA_SESSION="$2" "$CIRA" post >/dev/null)
}
post "$OUTER" fx-idle <<'J'
{"title":"fixture block one","items":[{"notify":"one"},{"ask":"keep?","kind":"verdict","default":"yes"}]}
J
post "$OUTER" fx-idle <<'J'
{"title":"fixture block two","items":[{"notify":"two"}]}
J
"$CIRA" status --json > "$HERE/status.json"
echo "wrote $HERE/status.json"
