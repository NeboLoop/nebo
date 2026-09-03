#!/usr/bin/env bash
# test-tools — deterministic tool cases over the running server's /agent/mcp
# endpoint. No model in the loop: each case is a JSON-RPC tools/call, a jq
# predicate on the result text, and a filesystem assertion. Exits non-zero on
# the first failure and prints the case id. Needs `make dev` up and jq.
#
#   make test-tools                 # all cases
#   make test-tools CASE=os-plan    # cases whose id starts with a prefix
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
# call '<arguments json>' [tool] — os tool by default; stores the result text in LAST and is_error in LAST_ERR
call() {
  local args="$1" tool="${2:-os}"
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
if case_ os-checkpoint-01 "checkpoint then restore round-trips bytes"; then
  printf one > "$WORK/a.txt"
  call "{\"resource\":\"file\",\"action\":\"checkpoint\",\"paths\":[\"$WORK/a.txt\",\"$WORK/new.txt\"],\"label\":\"t\"}"
  expect_ok "checkpoint cp-"; id=$(cp_id); [ -n "$id" ] || die "no checkpoint id in result"
  printf two > "$WORK/a.txt"; printf later > "$WORK/new.txt"
  call "{\"resource\":\"file\",\"action\":\"restore\",\"checkpoint\":\"$id\"}"
  expect_ok "restored checkpoint $id"
  expect_file "$WORK/a.txt" one; expect_absent "$WORK/new.txt"
  echo "$LAST" | grep -q "undo" || die "restore names its undo checkpoint"
  ok
fi

if case_ os-checkpoint-02 "subset restore touches only the named path"; then
  printf a1 > "$WORK/a.txt"; printf b1 > "$WORK/b.txt"
  call "{\"resource\":\"file\",\"action\":\"checkpoint\",\"paths\":[\"$WORK/a.txt\",\"$WORK/b.txt\"]}"
  expect_ok; id=$(cp_id)
  printf a2 > "$WORK/a.txt"; printf b2 > "$WORK/b.txt"
  call "{\"resource\":\"file\",\"action\":\"restore\",\"checkpoint\":\"$id\",\"paths\":[\"$WORK/b.txt\"]}"
  expect_ok; expect_file "$WORK/a.txt" a2; expect_file "$WORK/b.txt" b1
  ok
fi

if case_ os-checkpoint-03 "checkpoints lists what was taken"; then
  call '{"resource":"file","action":"checkpoints"}'
  expect_ok "cp-"; echo "$LAST" | grep -q "$WORK/a.txt" || die "listing names the file"
  ok
fi

if case_ os-checkpoint-04 "restore of an unknown id names the known ones"; then
  call '{"resource":"file","action":"restore","checkpoint":"cp-nope"}'
  expect_error "no checkpoint cp-nope"; echo "$LAST" | grep -q "Known: cp-" || die "known ids listed"
  ok
fi

if case_ os-checkpoint-05 "a directory is refused"; then
  call "{\"resource\":\"file\",\"action\":\"checkpoint\",\"paths\":[\"$WORK\"]}"
  expect_error "directory"
  ok
fi

if case_ os-checkpoint-06 "restore twice: the undo of an undo"; then
  printf v0 > "$WORK/c.txt"
  call "{\"resource\":\"file\",\"action\":\"checkpoint\",\"paths\":[\"$WORK/c.txt\"]}"; expect_ok; id=$(cp_id)
  printf v1 > "$WORK/c.txt"
  call "{\"resource\":\"file\",\"action\":\"restore\",\"checkpoint\":\"$id\"}"; expect_ok
  undo=$(echo "$LAST" | grep -o 'checkpoint cp-[0-9-]*-[a-f0-9]* — restore it\|is checkpoint cp-[0-9]\{8\}-[0-9]\{6\}-[0-9]\{4\}-[a-f0-9]\{6\}' | grep -o 'cp-[0-9]\{8\}-[0-9]\{6\}-[0-9]\{4\}-[a-f0-9]\{6\}' | tail -1)
  [ -n "$undo" ] || die "undo id not found"
  expect_file "$WORK/c.txt" v0
  call "{\"resource\":\"file\",\"action\":\"restore\",\"checkpoint\":\"$undo\"}"; expect_ok
  expect_file "$WORK/c.txt" v1
  ok
fi

# ---- plans ------------------------------------------------------------------
PLAN="$WORK/PLAN.md"
if case_ os-plan-01 "plan writes a marked document with N steps"; then
  call "{\"resource\":\"file\",\"action\":\"plan\",\"path\":\"$PLAN\",\"title\":\"Two steps\",\"steps\":[{\"title\":\"passes\",\"verify\":\"true\"},{\"title\":\"fails\",\"verify\":\"exit 3\"}]}"
  expect_ok "plan written"; grep -q "nebo-plan v1" "$PLAN" || die "marker missing"
  grep -q '^- \[ \] 1\. passes (verify: `true`)$' "$PLAN" || die "step 1 line shape"
  grep -q '—' "$PLAN" && die "em-dash in an owner-visible document"
  ok
fi

if case_ os-plan-02 "plan_check ticks only the passing step"; then
  call "{\"resource\":\"file\",\"action\":\"plan_check\",\"path\":\"$PLAN\"}"
  expect_ok "1/2 verified"
  grep -q '^- \[x\] 1\.' "$PLAN" || die "step 1 ticked"
  grep -q '^- \[ \] 2\.' "$PLAN" || die "step 2 not ticked"
  grep -q '2\. ✗ fails, exit 3' "$PLAN" || die "failing step carries its exit code"
  ok
fi

if case_ os-plan-03 "re-check is idempotent: one Last check block"; then
  call "{\"resource\":\"file\",\"action\":\"plan_check\",\"path\":\"$PLAN\"}"
  expect_ok; [ "$(grep -c '^Last check:' "$PLAN")" = 1 ] || die "Last check block duplicated"
  ok
fi

if case_ os-plan-04 "a plan without verify commands is refused"; then
  call "{\"resource\":\"file\",\"action\":\"plan\",\"path\":\"$WORK/P2.md\",\"title\":\"t\",\"steps\":[{\"title\":\"no verify\"}]}"
  expect_error "cannot be checked"; expect_absent "$WORK/P2.md"
  ok
fi

if case_ os-plan-05 "a check that verifies nothing is an error, not progress"; then
  call "{\"resource\":\"file\",\"action\":\"plan\",\"path\":\"$WORK/P3.md\",\"title\":\"t\",\"steps\":[{\"title\":\"fails\",\"verify\":\"false\"}]}"; expect_ok
  call "{\"resource\":\"file\",\"action\":\"plan_check\",\"path\":\"$WORK/P3.md\"}"
  expect_error "Nothing verified"
  ok
fi

if case_ os-plan-06 "a destructive verify command is refused and stays unticked"; then
  call "{\"resource\":\"file\",\"action\":\"plan\",\"path\":\"$WORK/P4.md\",\"title\":\"t\",\"steps\":[{\"title\":\"bad\",\"verify\":\"git stash\"},{\"title\":\"good\",\"verify\":\"true\"}]}"; expect_ok
  call "{\"resource\":\"file\",\"action\":\"plan_check\",\"path\":\"$WORK/P4.md\"}"
  expect_ok "1/2 verified"; grep -q '1\. ✗ bad, did not run' "$WORK/P4.md" || die "refused step reads 'did not run'"
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

# ---- destructive git ------------------------------------------------------
# Not here: the shell resource is denied from the MCP origin (policy deny list),
# so /agent/mcp cannot run `git stash`. The refusal table lives in
# crates/tools/src/policy.rs tests and the shell tool's own tests (L1).
# os-plan-06 above still proves the refusal reaches a verify command.

rm -rf "$WORK"
echo
echo "test-tools: $pass passed, $fail failed, $skipped skipped"
[ "$fail" = 0 ]
