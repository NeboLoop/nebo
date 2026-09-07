#!/usr/bin/env bash
# test-engine — the LIVE half of the engine proof, against a running Nebo
# with a real model. The deterministic half is `cargo test -p nebo-server --
# engine::proof` (sixty-four scenarios, no model). This script drives the
# same rules through the real doors — a source event, the case turn the
# engine starts, the ledger, the inspector — and asserts rows, never prose.
#
# It writes to a real contact: set ENGINE_CONTACT to an address YOU own.
# Every run uses a fresh plus-address (you+engine<ts>@…) so each run is a
# new person to the engine. Each turn is a real model call (about a minute
# and tens of thousands of tokens on the routed model).
#
#   make test-engine-live ENGINE_CONTACT=you@example.com
#   ENGINE_DB="…/nebo.db" — the server's database, read-only, for the
#     invariants that the API does not expose (undelivered events, the
#     ledger's one-message-per-person rule). Default: the macOS install.
#   ENGINE_TIMER=1 — also fire the case's next deadline now (ONE write to the
#     database: the timer's due_at) and prove the cadence turn starts.
#   NEBO_PID=<pid> NEBO_START='<command>' — also run the kill scenario: kill
#     the server the instant Mail takes a message, restart it with the
#     command, and prove the held send is reconciled and never sent twice.
#
# Exits non-zero on the first failed assertion and prints why.
set -u
TEST_SERVER="${TEST_SERVER:-localhost:27895}"
CONTACT="${ENGINE_CONTACT:-}"
AGENT="${ENGINE_AGENT:-intake-coordinator}"
SOURCE="${ENGINE_SOURCE:-sales.intake-coordinator.lead-captured}"
DB="${ENGINE_DB:-$HOME/Library/Application Support/Nebo/data/nebo.db}"
TURN_TIMEOUT="${ENGINE_TURN_TIMEOUT:-900}"

[ -n "$CONTACT" ] || { echo "FAIL: set ENGINE_CONTACT to an email address you own; the employee will write to it."; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq not found (brew install jq)."; exit 1; }
curl -sf -m 3 "http://$TEST_SERVER/health" >/dev/null || { echo "FAIL: no Nebo on $TEST_SERVER."; exit 1; }
HAVE_DB=0; [ -r "$DB" ] && command -v sqlite3 >/dev/null && HAVE_DB=1

RUN=$(date +%s)
LOCAL="${CONTACT%%@*}"; DOMAIN="${CONTACT#*@}"
EMAIL="${LOCAL}+engine${RUN}@${DOMAIN}"
pass=0; fail=0
ok()  { pass=$((pass+1)); echo "  ok   $1"; }
die() { fail=$((fail+1)); echo "  FAIL $1"; echo "       $2"; exit 1; }
api() { curl -s -m 30 "http://$TEST_SERVER$1"; }
sql() { sqlite3 -readonly "$DB" "$1"; }

emit() { # $1 email, $2 message
  local body
  body=$(jq -cn --arg s "$SOURCE" --arg e "$1" --arg m "$2" \
    '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:"nebo",arguments:{action:"emit",source:$s,payload:{email:$e,name:"Engine proof",source:"website-form",message:$m}}}}')
  curl -s -m 30 -X POST "http://$TEST_SERVER/agent/mcp" -H 'Content-Type: application/json' -d "$body" | jq -e '.result.isError == false' >/dev/null \
    || die "emit" "the event was not accepted by /agent/mcp"
}
case_for() { # $1 email → case id or empty
  api "/api/v1/cases?agent=$AGENT&limit=100" | jq -r --arg e "$(echo "$1" | tr 'A-Z' 'a-z')" \
    '[.cases[] | select(any(.aliases[]?; ascii_downcase | contains($e)))] | sort_by(.opened_at) | last | .id // empty'
}
detail() { api "/api/v1/cases/$1"; }
# A turn is settled when the engine has written its words to the case's
# history — the tick after the workflow ends — not merely when it ended.
settled() { detail "$1" | jq '[.history[] | select(.kind == "turn_result" or .kind == "turn_failed")] | length'; }
wait_for() { # $1 label, $2 seconds, $3 jq-free shell predicate (command string)
  local i; for i in $(seq 1 "$2"); do if eval "$3" >/dev/null 2>&1; then return 0; fi; sleep 1; done; return 1
}
wait_settled() { # $1 case, $2 count
  local i; for i in $(seq 1 $((TURN_TIMEOUT / 5))); do [ "$(settled "$1")" -ge "$2" ] && return 0; sleep 5; done; return 1
}

echo "engine live proof — server $TEST_SERVER, employee $AGENT, contact $EMAIL"

# S1 — a lead arrives: one case opens, its first turn runs on the real model,
# the turn ends with the contract, the case waits, and the history carries
# the ledger's receipts beside the turn's words.
echo "S1 a lead opens a case and the first turn keeps the contract"
emit "$EMAIL" "Hi, I need a quote for weekly lawn care on a half-acre lot. When could someone come by?"
wait_for "case" 30 "[ -n \"\$(case_for \"$EMAIL\")\" ]" || die "S1" "no case opened for $EMAIL within 30 s"
CASE=$(case_for "$EMAIL"); ok "case $CASE opened"
wait_settled "$CASE" 1 || die "S1" "the first turn did not settle within ${TURN_TIMEOUT}s"
D=$(detail "$CASE")
STATE=$(echo "$D" | jq -r '.case.state')
[ "$STATE" = "waiting" ] || [ "$STATE" = "done" ] || die "S1" "case state after the first turn: $STATE"
echo "$D" | jq -e '.history[] | select(.kind == "turn_result") | select(.text | test("on the ledger this turn"))' >/dev/null \
  || die "S1" "the history line has no receipts clause: $(echo "$D" | jq -c '.history')"
RECEIPTS=$(echo "$D" | jq '[.turns[0].receipts[] | select(.state == "completed")] | length')
ok "first turn settled; case $STATE; $RECEIPTS completed send(s) on the ledger"
[ "$STATE" = "waiting" ] || { echo "  note: the model closed the case on the first turn; the reply scenarios need a waiting case"; }

# S2 — the person replies: the case's own turn starts within seconds, and
# the deadline that was waiting is superseded, not fired.
if [ "$STATE" = "waiting" ]; then
  echo "S2 a reply wakes the case within seconds"
  WAKE_BEFORE=$(echo "$D" | jq -r '.case.waiting_for.wake_at')
  emit "$EMAIL" "Thanks — yes, Tuesday morning works. What does the first visit look like?"
  wait_for "turn" 20 "[ \"\$(detail \"$CASE\" | jq '.turns | length')\" -ge 2 ]" || die "S2" "no second turn within 20 s of the reply"
  ok "second turn started within 20 s"
  wait_settled "$CASE" 2 || die "S2" "the second turn did not settle"
  D=$(detail "$CASE")
  echo "$D" | jq -e --arg w "$WAKE_BEFORE" '[.waits[] | select((.wake_at|tostring) == $w)] | all(.superseded_at != null)' >/dev/null \
    || die "S2" "the old deadline is still live: $(echo "$D" | jq -c '.waits')"
  ok "the earlier deadline was superseded by the reply's turn"
  STATE=$(echo "$D" | jq -r '.case.state')
fi

# S3 — a message lands while a turn is running: it is never lost. Either the
# running turn heard it (steered, delivered by injection) or a turn carries
# it afterwards; in every case nothing stays undelivered.
if [ "$STATE" = "waiting" ]; then
  echo "S3 a message that lands mid-turn is never lost"
  emit "$EMAIL" "One more thing: my gate code is 4471."
  wait_for "turn" 20 "[ \"\$(detail \"$CASE\" | jq '[.turns[] | select(.state == \"running\" or .state == \"queued\")] | length')\" -ge 1 ]" || die "S3" "no turn started for the message"
  sleep 3
  emit "$EMAIL" "Correction: the gate code is 4417, not 4471."
  N=$(detail "$CASE" | jq '.turns | length')
  wait_settled "$CASE" "$N" || die "S3" "the turn did not settle"
  sleep 130   # a deferred message rides the next tick after its lease (120 s)
  wait_settled "$CASE" "$(detail "$CASE" | jq '.turns | length')" || die "S3" "a follow-up turn did not settle"
  if [ "$HAVE_DB" = 1 ]; then
    LEFT=$(sql "select count(*) from engine_events where kind='signal' and delivered_at is null")
    [ "$LEFT" = "0" ] || die "S3" "$LEFT signal(s) still undelivered"
    ok "every signal delivered; nothing lost"
  else
    ok "turns settled after the mid-turn message (database not readable: undelivered check skipped)"
  fi
  D=$(detail "$CASE"); STATE=$(echo "$D" | jq -r '.case.state')
fi

# S4 — the deadline fires the cadence turn (one write: the timer's due_at).
if [ "${ENGINE_TIMER:-0}" = 1 ] && [ "$STATE" = "waiting" ] && [ "$HAVE_DB" = 1 ]; then
  echo "S4 the deadline starts the next turn"
  W=$(detail "$CASE" | jq -r '.case.waiting_for.id')
  N=$(detail "$CASE" | jq '.turns | length')
  sqlite3 "$DB" "update engine_events set due_at=$(date +%s) where kind='timer' and target_type='wait' and target_id='$W' and delivered_at is null"
  wait_for "turn" 20 "[ \"\$(detail \"$CASE\" | jq '.turns | length')\" -gt $N ]" || die "S4" "the timer did not start a turn within 20 s"
  ok "the timer started a turn"
  wait_settled "$CASE" $((N + 1)) || die "S4" "the cadence turn did not settle"
  D=$(detail "$CASE"); STATE=$(echo "$D" | jq -r '.case.state')
fi

# S5 — kill the server the instant Mail takes a message; restart; the held
# send is reconciled, the owner told, and the person never gets two.
if [ -n "${NEBO_PID:-}" ] && [ -n "${NEBO_START:-}" ] && [ "$HAVE_DB" = 1 ]; then
  echo "S5 a kill mid-send is held, reconciled, and never sent twice"
  KEMAIL="${LOCAL}+engine${RUN}k@${DOMAIN}"
  emit "$KEMAIL" "Hello, I'd like a quote for a spring cleanup."
  killed=0
  for i in $(seq 1 3000); do
    if pgrep -f 'send newMsg' >/dev/null 2>&1; then kill -9 "$NEBO_PID"; killed=1; break; fi
    sleep 0.1
  done
  [ "$killed" = 1 ] || die "S5" "never saw a send within 300 s"
  sleep 2; bash -c "$NEBO_START"
  wait_for "health" 60 "curl -sf -m 2 http://$TEST_SERVER/health" || die "S5" "the server did not come back"
  KCASE=$(case_for "$KEMAIL"); [ -n "$KCASE" ] || die "S5" "no case for $KEMAIL"
  wait_settled "$KCASE" 1 || die "S5" "the relaunched turn did not settle"
  DOUBLE=$(sql "select count(*) from (select run_id, counterparty from engine_effects where state='completed' and counterparty is not null group by run_id, counterparty having count(*) > 1)")
  [ "$DOUBLE" = "0" ] || die "S5" "a run sent the same person more than once"
  HELD=$(sql "select count(*) from engine_effects where state='pending' and attempts>0")
  [ "$HELD" -ge 1 ] || die "S5" "no held send on the ledger after the kill"
  CARD=$(sql "select count(*) from notifications where id like 'attention:effect:%'")
  [ "$CARD" -ge 1 ] || die "S5" "the owner was not told about the held send"
  ok "held send reconciled, owner told, no second message to the person"
fi

# Invariants over the whole store, whatever the scenarios did.
if [ "$HAVE_DB" = 1 ]; then
  echo "invariants"
  DOUBLE=$(sql "select count(*) from (select run_id, counterparty from engine_effects where state='completed' and counterparty is not null group by run_id, counterparty having count(*) > 1)")
  [ "$DOUBLE" = "0" ] || die "invariant" "some run sent one person twice"
  ok "one message per person per run, everywhere"
  STALE=$(sql "select count(*) from engine_events where kind='signal' and delivered_at is null and created_at < $(date +%s) - 600")
  [ "$STALE" = "0" ] || die "invariant" "$STALE signal(s) undelivered for over ten minutes"
  ok "no signal undelivered for more than ten minutes"
  TWO=$(sql "select count(*) from (select r.parent_run_id from engine_runs r where r.kind='workflow' and r.state in ('queued','running') and r.parent_run_id is not null group by r.parent_run_id having count(*) > 1)")
  [ "$TWO" = "0" ] || die "invariant" "a case has two live turns"
  ok "one live turn per case"
fi

echo "engine live proof: $pass ok, $fail failed — case $CASE for $EMAIL"
exit 0
