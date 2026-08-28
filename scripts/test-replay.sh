#!/usr/bin/env bash
# test-replay — synthetic poisoned-window recovery test (offline-safe seeding).
#
# Seeds a fully SYNTHETIC poisoned conversation (forged "[os] 0 lines" tool
# results + stuck-in-a-loop narration) straight into the live server's DB,
# dispatches ONE real message over /ws, and asserts the agent recovers: it
# actually reads /tmp/nebo-replay/project/main.py and describes it, without
# spiralling or parroting the poison.
#
# PASS (per run): chat_complete arrives, 1 <= tool_start count <= 12, and the
#                 streamed text contains none of: "stuck in a loop",
#                 "can't read", "cannot read".
# FAIL:           tool_start count > 20 (cancelled by run_id) or 240s elapse.
#
# Env: TEST_SERVER (default 127.0.0.1:27895), REPLAY_DB (default per-OS),
#      REPLAY_RUNS (default 1; pass bar = ALL runs pass).
set -u

TEST_SERVER="${TEST_SERVER:-127.0.0.1:27895}"
REPLAY_RUNS="${REPLAY_RUNS:-1}"
DEADLINE_SECS=240
FAIL_TOOL_CALLS=20
PASS_TOOL_CALLS=12
MAIN_PY="/tmp/nebo-replay/project/main.py"

if [ -z "${REPLAY_DB:-}" ]; then
  if [ "$(uname -s)" = "Darwin" ]; then
    REPLAY_DB="$HOME/Library/Application Support/Nebo/data/nebo.db"
  else
    REPLAY_DB="$HOME/.nebo/data/nebo.db"
  fi
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
GEN="$SCRIPT_DIR/gen-poisoned-window.py"

# ── Preflight ────────────────────────────────────────────────────────────────
[ -f "$REPLAY_DB" ] || { echo "FAIL: DB not found at '$REPLAY_DB' (set REPLAY_DB). Refusing to run."; exit 1; }
command -v websocat >/dev/null || { echo "FAIL: websocat not found (brew install websocat)."; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 not found."; exit 1; }
curl -sf -m 3 "http://$TEST_SERVER/health" >/dev/null \
  || { echo "FAIL: no Nebo on $TEST_SERVER — start one with 'make dev' first."; exit 1; }

overall_pass=1

# extract_run_id <events-file> <session-key>
# From "active_runs" replies (WS list_active_runs), find our run's runId.
extract_run_id() {
  python3 - "$1" "$2" <<'PYEOF'
import json, sys
path, session = sys.argv[1], sys.argv[2]
run_id = ""
try:
    for line in open(path, errors="replace"):
        line = line.strip()
        if '"active_runs"' not in line:
            continue
        try:
            ev = json.loads(line)
        except Exception:
            continue
        if ev.get("type") != "active_runs":
            continue
        for r in ev.get("data", {}).get("runs", []):
            if r.get("sessionKey") == session:
                run_id = r.get("runId", "")
except FileNotFoundError:
    pass
print(run_id)
PYEOF
}

# analyze <events-file> <session-key> → JSON verdict on stdout
analyze() {
  python3 - "$1" "$2" <<'PYEOF'
import json, sys
path, session = sys.argv[1], sys.argv[2]
tool_starts = 0
complete = False
error = ""
text_parts = []
try:
    for line in open(path, errors="replace"):
        line = line.strip()
        if not line:
            continue
        try:
            ev = json.loads(line)
        except Exception:
            continue
        data = ev.get("data") or {}
        if data.get("session_id") != session:
            continue
        t = ev.get("type")
        if t == "tool_start":
            tool_starts += 1
        elif t == "chat_complete":
            complete = True
        elif t == "chat_error":
            error = str(data.get("error", "unknown"))
        elif t == "chat_stream":
            text_parts.append(str(data.get("content") or data.get("text") or ""))
except FileNotFoundError:
    pass
text = "".join(text_parts)
lower = text.lower()
forbidden = [p for p in ("stuck in a loop", "can't read", "cannot read") if p in lower]
print(json.dumps({
    "tool_starts": tool_starts,
    "complete": complete,
    "error": error,
    "text_len": len(text),
    "forbidden": forbidden,
}))
PYEOF
}

run_once() {
  local run_no="$1"
  local uuid session title info workdir events fifo ws_pid started elapsed
  uuid="$(python3 -c 'import uuid; print(uuid.uuid4())')"
  session="agent:assistant:thread:$uuid"
  title="REPLAY $(date +%Y%m%d-%H%M%S)"

  echo "── run $run_no/$REPLAY_RUNS ─ session $session"
  info="$(python3 "$GEN" --db "$REPLAY_DB" --chat-id "$uuid" --session-key "$session" --title "$title" --main-py "$MAIN_PY")" \
    || { echo "FAIL: seeding failed"; return 1; }
  echo "  seeded: $info"

  workdir="$(mktemp -d "${TMPDIR:-/tmp}/nebo-replay-run.XXXXXX")"
  events="$workdir/events.jsonl"
  fifo="$workdir/control"
  mkfifo "$fifo"

  websocat -n -t "ws://$TEST_SERVER/ws" < "$fifo" > "$events" 2>"$workdir/ws.err" &
  ws_pid=$!
  # keep the fifo (and the socket) open for the whole watch
  exec 3>"$fifo"

  printf '%s\n' '{"type":"connect"}' >&3
  sleep 1
  printf '{"type":"chat","message_id":"replay-%s","data":{"session_id":"%s","prompt":"can you read %s and tell me what it does?","user_id":"replay","channel":"web"}}\n' \
    "$run_no" "$session" "$MAIN_PY" >&3

  started=$(date +%s)
  local verdict="" reason="" run_id="" analysis tool_starts complete
  while :; do
    sleep 3
    # refresh the active-run listing so run_id is on file when we need it
    printf '%s\n' '{"type":"list_active_runs"}' >&3 2>/dev/null || true

    analysis="$(analyze "$events" "$session")"
    tool_starts=$(printf '%s' "$analysis" | python3 -c 'import json,sys; print(json.load(sys.stdin)["tool_starts"])')
    complete=$(printf '%s' "$analysis" | python3 -c 'import json,sys; print(1 if json.load(sys.stdin)["complete"] else 0)')
    elapsed=$(( $(date +%s) - started ))

    if [ "$tool_starts" -gt "$FAIL_TOOL_CALLS" ]; then
      verdict="FAIL"; reason="tool_start count $tool_starts exceeded $FAIL_TOOL_CALLS — runaway"
      run_id="$(extract_run_id "$events" "$session")"
      if [ -n "$run_id" ]; then
        printf '{"type":"cancel","data":{"run_id":"%s"}}\n' "$run_id" >&3
        echo "  cancelled run $run_id (by run_id)"
        sleep 2
      else
        # NEVER cancel by session_id alone: an unmatched session_id cancel
        # kills ALL runs on the box. Leave the run; report loudly.
        echo "  WARNING: run_id not found in active_runs — NOT sending cancel (never cancel by session_id)"
      fi
      break
    fi
    if [ "$complete" = "1" ]; then
      break
    fi
    if [ "$elapsed" -ge "$DEADLINE_SECS" ]; then
      verdict="FAIL"; reason="deadline: ${DEADLINE_SECS}s elapsed without chat_complete"
      run_id="$(extract_run_id "$events" "$session")"
      if [ -n "$run_id" ]; then
        printf '{"type":"cancel","data":{"run_id":"%s"}}\n' "$run_id" >&3
        echo "  cancelled run $run_id (by run_id)"
        sleep 2
      else
        echo "  WARNING: run_id not found in active_runs — NOT sending cancel (never cancel by session_id)"
      fi
      break
    fi
  done

  exec 3>&-
  kill "$ws_pid" 2>/dev/null
  wait "$ws_pid" 2>/dev/null

  analysis="$(analyze "$events" "$session")"
  echo "  events: $analysis"
  elapsed=$(( $(date +%s) - started ))

  if [ -z "$verdict" ]; then
    verdict="$(printf '%s' "$analysis" | python3 -c "
import json, sys
a = json.load(sys.stdin)
ok = (a['complete']
      and 1 <= a['tool_starts'] <= $PASS_TOOL_CALLS
      and not a['forbidden']
      and not a['error']
      and a['text_len'] > 0)
print('PASS' if ok else 'FAIL')")"
    if [ "$verdict" = "FAIL" ]; then
      reason="$(printf '%s' "$analysis" | python3 -c "
import json, sys
a = json.load(sys.stdin)
why = []
if not a['complete']: why.append('no chat_complete')
if a['error']: why.append('chat_error: ' + a['error'])
if a['tool_starts'] < 1: why.append('no tool activity at all (empty/errored transcript is a FAIL)')
if a['tool_starts'] > $PASS_TOOL_CALLS: why.append(f\"tool_starts {a['tool_starts']} > $PASS_TOOL_CALLS\")
if a['forbidden']: why.append('streamed text contains: ' + ', '.join(a['forbidden']))
if a['text_len'] == 0: why.append('no streamed text')
print('; '.join(why) or 'unknown')")"
    fi
  fi

  local stats
  stats="$(printf '%s' "$analysis" | python3 -c "
import json, sys
a = json.load(sys.stdin)
print(f\"tool_calls={a['tool_starts']} text_chars={a['text_len']} complete={a['complete']}\")")"
  echo "  $verdict — $stats duration=${elapsed}s ${reason:+reason: $reason}"
  echo "  events file: $events"

  [ "$verdict" = "PASS" ]
}

echo "test-replay: server=$TEST_SERVER db=$REPLAY_DB runs=$REPLAY_RUNS"
pass_count=0
for i in $(seq 1 "$REPLAY_RUNS"); do
  if run_once "$i"; then
    pass_count=$((pass_count + 1))
  else
    overall_pass=0
  fi
done

echo ""
if [ "$overall_pass" = "1" ]; then
  echo "VERDICT: PASS ($pass_count/$REPLAY_RUNS runs recovered from the poisoned window)"
  exit 0
else
  echo "VERDICT: FAIL ($pass_count/$REPLAY_RUNS runs passed; pass bar is all runs)"
  exit 1
fi
