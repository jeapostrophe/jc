#!/bin/sh
# Regenerates status.json, the `cira status --json` fixture of jc_core::cira's tests, from a
# real Cira daemon: a scratch store (never ~/.cira), its own port, hook calls that put one
# session in each state, and two posted blocks.
#
#   make-status.sh /path/to/cira
#
# Each session's CIRA_SESSION is `fx-<state>`, so a test can read the state a session was
# driven to from its id. Projects `outer` and `outer/inner` nest, for the deepest-path match.
# Project `owned` holds sessions Cira runs (`cira session new`, with a fake `claude` that keeps
# its jobs as files), each labelled by what was done to it: running, dangerous, stopped, removed.
set -eu
# This shell may itself run inside a Claude Code session: none of its identity may leak in.
unset CLAUDE_CODE_SESSION_ID CLAUDE_PID CLAUDE_JOB_DIR TMUX_PANE CLAUDE_CODE_MESSAGING_SOCKET CIRA_SESSION
CIRA=${1:?usage: make-status.sh /path/to/cira}
HERE=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d /tmp/jcfx.XXXXXX)
export CIRA_HOME="$T/home" CIRA_NO_LIVE_HOME=1
mkdir -p "$CIRA_HOME" "$T/outer/inner" "$T/owned" "$T/fake/config/projects"
OUTER=$(cd "$T/outer" && pwd -P)
INNER="$OUTER/inner"
OWNED=$(cd "$T/owned" && pwd -P)
# The fake `claude` the daemon runs for `cira session` (Cira's own tests use the same shape).
cat > "$T/fake/claude" <<'FAKE'
#!/bin/sh
d=$(dirname "$0"); mkdir -p "$d/jobs"
case "$1" in
  agents)
    printf '['; sep=''
    for f in "$d"/jobs/*.state; do
      [ -e "$f" ] || continue
      id=$(basename "$f" .state); st=$(cat "$f")
      if [ "$st" = live ]; then printf '%s{"pid":7,"id":"%s","kind":"background","sessionId":"%s-0000-4000-8000-000000000000","status":"idle"}' "$sep" "$id" "$id"
      else printf '%s{"id":"%s","kind":"background","sessionId":"%s-0000-4000-8000-000000000000","state":"%s"}' "$sep" "$id" "$id" "$st"; fi
      sep=','
    done
    echo ']' ;;
  --bg) n=$(cat "$d/count" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$d/count"
    id=$(printf 'bbbb%04d' $n); echo live > "$d/jobs/$id.state"; echo "backgrounded · $id · name" ;;
  stop) echo stopped > "$d/jobs/$2.state" ;;
  rm) rm -f "$d/jobs/$2.state" ;;
esac
FAKE
printf '#!/bin/sh\n[ "$1" = -l ] && shift\nexec /bin/sh "$@"\n' > "$T/fake/shell"
chmod +x "$T/fake/claude" "$T/fake/shell"
CIRA_CLAUDE="$T/fake/claude" SHELL="$T/fake/shell" CLAUDE_CONFIG_DIR="$T/fake/config" \
  "$CIRA" daemon --port 0 > "$T/daemon.out" 2>&1 &
DPID=$!
trap 'kill $DPID 2>/dev/null; wait $DPID 2>/dev/null; rm -rf "$T"' EXIT
i=0; until "$CIRA" ping >/dev/null 2>&1; do i=$((i+1)); [ $i -lt 100 ] || { cat "$T/daemon.out"; exit 1; }; sleep 0.1; done
"$CIRA" project add outer --path "$OUTER" >/dev/null
"$CIRA" project add inner --path "$INNER" >/dev/null
"$CIRA" project add owned --path "$OWNED" >/dev/null

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
# new SESSION-LABEL [FLAGS]: a session Cira runs, its id printed once its job is bound.
new() { label=$1; shift; "$CIRA" session new --cwd "$OWNED" --label "$label" --wait --json "$@" |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["session"])'; }
new owned-running --auto >/dev/null
new owned-dangerous --dangerous >/dev/null
s=$(new owned-stopped); "$CIRA" session stop "$s" --project owned >/dev/null
s=$(new owned-removed); "$CIRA" session rm "$s" --project owned >/dev/null
# The stop and the removal are carried out by the daemon: wait until status shows both done.
i=0; until "$CIRA" status --json | python3 -c '
import json,sys
ss={s["label"]:s for p in json.load(sys.stdin)["projects"] for s in p["sessions"]}
ok=ss["owned-stopped"]["job"]["state"]=="stopped" and ss["owned-removed"]["job"]["state"]=="absent"
sys.exit(0 if ok else 1)'; do i=$((i+1)); [ $i -lt 100 ] || { echo "jobs did not settle"; exit 1; }; sleep 0.2; done
"$CIRA" status --json > "$HERE/status.json"
echo "wrote $HERE/status.json"
