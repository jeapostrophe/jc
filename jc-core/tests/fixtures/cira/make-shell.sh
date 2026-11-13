#!/bin/sh
# Regenerates session-shell.json, the `cira session shell --json` fixture of jc_core's tests,
# from a real Cira daemon: a scratch store (never ~/.cira), its own port, one session made with a
# fake `claude`, and its shell in that store's own tmux server (ended on exit).
#
#   make-shell.sh /path/to/cira
set -eu
unset CLAUDE_CODE_SESSION_ID CLAUDE_PID CLAUDE_JOB_DIR TMUX TMUX_PANE CLAUDE_CODE_MESSAGING_SOCKET CIRA_SESSION
CIRA=${1:?usage: make-shell.sh /path/to/cira}
HERE=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d /tmp/jcsh.XXXXXX)
export CIRA_HOME="$T/home" CIRA_NO_LIVE_HOME=1
mkdir -p "$CIRA_HOME" "$T/proj" "$T/fake/config/projects"
PROJ=$(cd "$T/proj" && pwd -P)
cat > "$T/fake/claude" <<'FAKE'
#!/bin/sh
d=$(dirname "$0")
case "$1" in
  agents) if [ -e "$d/job" ]; then echo '[{"pid":7,"id":"bbbb0001","kind":"background","sessionId":"bbbb0001-0000-4000-8000-000000000000","status":"idle"}]'; else echo '[]'; fi ;;
  --bg) touch "$d/job"; echo "backgrounded · bbbb0001 · name" ;;
esac
FAKE
printf '#!/bin/sh\n[ "$1" = -l ] && shift\nexec /bin/sh "$@"\n' > "$T/fake/shell"
chmod +x "$T/fake/claude" "$T/fake/shell"
CIRA_CLAUDE="$T/fake/claude" SHELL="$T/fake/shell" CLAUDE_CONFIG_DIR="$T/fake/config" \
  "$CIRA" daemon --port 0 > "$T/daemon.out" 2>&1 &
DPID=$!
trap 'tmux -S "$CIRA_HOME/tmux.sock" kill-server 2>/dev/null; kill $DPID 2>/dev/null; wait $DPID 2>/dev/null; rm -rf "$T"' EXIT
i=0; until "$CIRA" ping >/dev/null 2>&1; do i=$((i+1)); [ $i -lt 100 ] || { cat "$T/daemon.out"; exit 1; }; sleep 0.1; done
"$CIRA" project add fx --path "$PROJ" >/dev/null
S=$("$CIRA" session new --cwd "$PROJ" --label fx-shell --wait --json |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["session"])')
"$CIRA" session shell "$S" --project fx --json > "$HERE/session-shell.json"
echo "wrote $HERE/session-shell.json"
