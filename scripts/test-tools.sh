#!/usr/bin/env bash
# test-tools — deterministic tool cases over the running server's /agent/mcp
# endpoint. No model in the loop: each case is a JSON-RPC tools/call, a jq
# predicate on the result text, and a filesystem assertion. Exits non-zero on
# the first failure and prints the case id. Needs `make dev` up and jq.
#
#   make test-tools                 # all cases
#   make test-tools CASE=plan       # cases whose id starts with a prefix
#
# Layer L2 of docs/plans/2026-09-02-parity-test-plan.md.
set -u

TEST_SERVER="${TEST_SERVER:-localhost:27895}"
ONLY="${CASE:-}"
WORK="${TMPDIR:-/tmp}"; WORK="${WORK%/}/nebo-test-tools"   # TMPDIR carries a trailing slash on macOS
command -v jq >/dev/null || { echo "FAIL: jq not found (brew install jq)."; exit 1; }
curl -sf -m 3 "http://$TEST_SERVER/health" >/dev/null \
  || { echo "FAIL: no Nebo on $TEST_SERVER — start one with 'make dev' first."; exit 1; }

pass=0; fail=0; skipped=0; current=""; LAST=""

# ---- helpers ---------------------------------------------------------------
case_() { current="$1"; if [ -n "$ONLY" ] && [[ "$1" != "$ONLY"* ]]; then skipped=$((skipped+1)); return 1; fi; printf '%-46s %s\n' "$1" "$2"; return 0; }
die()   { echo "  FAIL [$current]: $*"; fail=$((fail+1)); echo "  last result: ${LAST:0:600}"; exit 1; }
ok()    { pass=$((pass+1)); }
# call '<arguments json>' <tool> — stores the result text in LAST and is_error in LAST_ERR.
# /agent/mcp runs a tool by name, so a deferred tool needs no find_tools load here.
call() {
  local args="$1" tool="${2:?call needs a tool name}"
  local body
  body=$(jq -cn --argjson a "$args" --arg t "$tool" '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:$t,arguments:$a}}')
  local resp
  resp=$(curl -s -m 180 -X POST "http://$TEST_SERVER/agent/mcp" -H 'Content-Type: application/json' -d "$body")
  LAST=$(echo "$resp" | jq -r '.result.content[0].text // .error.message // .')
  LAST_ERR=$(echo "$resp" | jq -r '.result.isError // false')
}
expect_ok()    { [ "$LAST_ERR" = "false" ] || die "expected success, got error"; [ -z "${1:-}" ] || echo "$LAST" | grep -q -- "$1" || die "expected text '$1'"; }
expect_error() { [ "$LAST_ERR" = "true" ] || die "expected an error"; [ -z "${1:-}" ] || echo "$LAST" | grep -q -- "$1" || die "expected error text '$1'"; }
expect_file()  { [ -f "$1" ] || die "$1 missing"; [ "$(cat "$1")" = "$2" ] || die "$1 = '$(cat "$1")', want '$2'"; }
expect_absent(){ [ ! -e "$1" ] || die "$1 should not exist"; }
cp_id()        { echo "$LAST" | grep -o 'cp-[0-9]\{8\}-[0-9]\{6\}-[0-9]\{4\}-[a-f0-9]\{6\}' | head -1; }

rm -rf "$WORK"; mkdir -p "$WORK"

# ---- checkpoints ------------------------------------------------------------
if case_ checkpoint-01 "checkpoint then restore round-trips bytes"; then
  printf one > "$WORK/a.txt"
  call "{\"paths\":[\"$WORK/a.txt\",\"$WORK/new.txt\"],\"label\":\"t\"}" checkpoint_files
  expect_ok "checkpoint cp-"; id=$(cp_id); [ -n "$id" ] || die "no checkpoint id in result"
  printf two > "$WORK/a.txt"; printf later > "$WORK/new.txt"
  call "{\"checkpoint\":\"$id\"}" restore_checkpoint
  expect_ok "restored checkpoint $id"
  expect_file "$WORK/a.txt" one; expect_absent "$WORK/new.txt"
  echo "$LAST" | grep -q "undo" || die "restore names its undo checkpoint"
  ok
fi

if case_ checkpoint-02 "subset restore touches only the named path"; then
  printf a1 > "$WORK/a.txt"; printf b1 > "$WORK/b.txt"
  call "{\"paths\":[\"$WORK/a.txt\",\"$WORK/b.txt\"]}" checkpoint_files
  expect_ok; id=$(cp_id)
  printf a2 > "$WORK/a.txt"; printf b2 > "$WORK/b.txt"
  call "{\"checkpoint\":\"$id\",\"paths\":[\"$WORK/b.txt\"]}" restore_checkpoint
  expect_ok; expect_file "$WORK/a.txt" a2; expect_file "$WORK/b.txt" b1
  ok
fi

if case_ checkpoint-03 "list_checkpoints lists what was taken"; then
  call '{}' list_checkpoints
  expect_ok "cp-"; echo "$LAST" | grep -q "$WORK/a.txt" || die "listing names the file"
  ok
fi

if case_ checkpoint-04 "restore of an unknown id names the known ones"; then
  call '{"checkpoint":"cp-nope"}' restore_checkpoint
  expect_error "no checkpoint cp-nope"; echo "$LAST" | grep -q "Known: cp-" || die "known ids listed"
  ok
fi

if case_ checkpoint-05 "a directory is refused"; then
  call "{\"paths\":[\"$WORK\"]}" checkpoint_files
  expect_error "directory"
  ok
fi

if case_ checkpoint-06 "restore twice: the undo of an undo"; then
  printf v0 > "$WORK/c.txt"
  call "{\"paths\":[\"$WORK/c.txt\"]}" checkpoint_files; expect_ok; id=$(cp_id)
  printf v1 > "$WORK/c.txt"
  call "{\"checkpoint\":\"$id\"}" restore_checkpoint; expect_ok
  undo=$(echo "$LAST" | grep -o 'checkpoint cp-[0-9-]*-[a-f0-9]* — restore it\|is checkpoint cp-[0-9]\{8\}-[0-9]\{6\}-[0-9]\{4\}-[a-f0-9]\{6\}' | grep -o 'cp-[0-9]\{8\}-[0-9]\{6\}-[0-9]\{4\}-[a-f0-9]\{6\}' | tail -1)
  [ -n "$undo" ] || die "undo id not found"
  expect_file "$WORK/c.txt" v0
  call "{\"checkpoint\":\"$undo\"}" restore_checkpoint; expect_ok
  expect_file "$WORK/c.txt" v1
  ok
fi

# ---- plans ------------------------------------------------------------------
PLAN="$WORK/PLAN.md"
if case_ plan-01 "write_plan writes a marked document with N steps"; then
  call "{\"path\":\"$PLAN\",\"title\":\"Two steps\",\"steps\":[{\"title\":\"passes\",\"verify\":\"true\"},{\"title\":\"fails\",\"verify\":\"exit 3\"}]}" write_plan
  expect_ok "plan written"; grep -q "nebo-plan v1" "$PLAN" || die "marker missing"
  grep -q '^- \[ \] 1\. passes (verify: `true`)$' "$PLAN" || die "step 1 line shape"
  grep -q '—' "$PLAN" && die "em-dash in an owner-visible document"
  ok
fi

if case_ plan-02 "check_plan ticks only the passing step"; then
  call "{\"path\":\"$PLAN\"}" check_plan
  expect_ok "1 of 2 steps pass"
  grep -q '^- \[x\] 1\.' "$PLAN" || die "step 1 ticked"
  grep -q '^- \[ \] 2\.' "$PLAN" || die "step 2 not ticked"
  grep -q '2\. ✗ fails, exit 3' "$PLAN" || die "failing step carries its exit code"
  ok
fi

if case_ plan-03 "re-check is idempotent: one Last check block"; then
  call "{\"path\":\"$PLAN\"}" check_plan
  expect_ok; [ "$(grep -c '^Last check:' "$PLAN")" = 1 ] || die "Last check block duplicated"
  ok
fi

if case_ plan-04 "a plan without verify commands is refused"; then
  call "{\"path\":\"$WORK/P2.md\",\"title\":\"t\",\"steps\":[{\"title\":\"no verify\"}]}" write_plan
  expect_error "cannot be checked"; expect_absent "$WORK/P2.md"
  ok
fi

if case_ plan-05 "a check that verifies nothing is an error, not progress"; then
  call "{\"path\":\"$WORK/P3.md\",\"title\":\"t\",\"steps\":[{\"title\":\"fails\",\"verify\":\"false\"}]}" write_plan; expect_ok
  call "{\"path\":\"$WORK/P3.md\"}" check_plan
  expect_error "Nothing verified"
  ok
fi

if case_ plan-06 "a destructive verify command is refused and stays unticked"; then
  call "{\"path\":\"$WORK/P4.md\",\"title\":\"t\",\"steps\":[{\"title\":\"bad\",\"verify\":\"git stash\"},{\"title\":\"good\",\"verify\":\"true\"}]}" write_plan; expect_ok
  call "{\"path\":\"$WORK/P4.md\"}" check_plan
  expect_ok "1 of 2 steps pass"; grep -q '1\. ✗ bad, did not run' "$WORK/P4.md" || die "refused step reads 'did not run'"
  ok
fi

# ---- sub-agent continuation (Stage 9) ---------------------------------------
# The live continuation itself is fixtures/tools/agent-send-continuation.yaml
# (a model in the loop). Here: the verb exists and its two refusals say what
# to do next, so a model never spirals on them.
if case_ agent-send-01 "send without a message names the missing parameter"; then
  call '{"resource":"task","action":"send","task_id":"sa-x"}' agent
  expect_error "message"; echo "$LAST" | grep -q 'action: "send"' || die "usage example shown"
  ok
fi

if case_ agent-send-02 "send to an unknown task says to spawn afresh"; then
  call '{"resource":"task","action":"send","task_id":"sa-nope","message":"more"}' agent
  expect_error "No sub-agent sa-nope to continue"; echo "$LAST" | grep -q "Spawn a new one" || die "recovery named"
  ok
fi

# ---- one door to a skill (Rule 8) -----------------------------------------
# A plugin's usage lives in its skills, and there is exactly ONE way to find
# and read one: the skill tool. 2026-09-15: the plugin tool had its own
# `help` action printing a trimmed label (`products`), the skill tool had its
# own `help` preview and a `catalog` alias of `list`, and an employee bounced
# between all four, concluded the docs were stale, and guessed GraphQL for 51
# calls. These cases keep the second doors shut.
if case_ skill-onedoor-01 "a plugin's skill loads through the skill tool"; then
  call '{"action":"list"}' skill
  expect_ok
  SKILL=$(echo "$LAST" | grep -o '\b[a-z0-9]\{2,\}-[a-z0-9-]\{2,\}\b' | head -1)
  if [ -z "$SKILL" ]; then
    echo "  (no skills installed — nothing to load)"; ok
  else
    call "$(jq -cn --arg n "$SKILL" '{action:"load",name:$n}')" skill
    expect_ok
    ok
  fi
fi

if case_ skill-onedoor-02 "skill help is gone; the answer names load"; then
  call '{"action":"help","name":"anything"}' skill
  expect_error "Unknown action"
  echo "$LAST" | grep -q "use load" || die "the recovery must name load"
  ok
fi

if case_ skill-onedoor-03 "skill catalog is gone; list is the one word"; then
  call '{"action":"catalog"}' skill
  expect_error "Unknown action"
  echo "$LAST" | grep -q "list" || die "the recovery must name list"
  ok
fi

if case_ plugin-onedoor-01 "plugin help is gone; skills are read through the skill tool"; then
  call '{"action":"help","resource":"anything"}' plugin
  expect_error 'skill(action'
  ok
fi

# ---- destructive git ------------------------------------------------------
# Not here: run_command is denied from the MCP origin (policy deny list),
# so /agent/mcp cannot run `git stash`. The refusal table lives in
# crates/tools/src/policy.rs tests and run_command's own tests (L1).
# plan-06 above still proves the refusal reaches a verify command.

rm -rf "$WORK"
echo
echo "test-tools: $pass passed, $fail failed, $skipped skipped"
[ "$fail" = 0 ]
