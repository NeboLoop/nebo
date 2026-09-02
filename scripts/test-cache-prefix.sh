#!/usr/bin/env bash
# test-cache-prefix — layer L6 of docs/plans/2026-09-02-parity-test-plan.md.
#
# Runs one multi-iteration coding fixture against the live server and reads
# the provider-reported prompt-cache tokens back out of the trace:
#   ratio = cache_read / (input + cache_read)
# A stable prefix (tools in name order, nothing volatile above CACHE_BOUNDARY)
# shows as a high ratio; a prefix that changes every iteration shows as ~0.
#
#   make test-cache-prefix                       # default fixture, bar 0.85
#   FIXTURE=fixtures/tools/os-checkpoint.yaml BAR=0.7 bash scripts/test-cache-prefix.sh
set -u
TEST_SERVER="${TEST_SERVER:-localhost:27895}"
FIXTURE="${FIXTURE:-fixtures/longsession/compaction-large-reads.yaml}"
BAR="${BAR:-0.85}"
OUT="${OUT:-/tmp/nebo-traces-cache}"
CLI="${NEBO_CLI:-./target/debug/nebo-cli}"

curl -sf -m 3 "http://$TEST_SERVER/health" >/dev/null \
  || { echo "FAIL: no Nebo on $TEST_SERVER — start one with 'make dev' first."; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq not found."; exit 1; }
[ -x "$CLI" ] || { echo "FAIL: $CLI not built (cargo build -p nebo-cli)."; exit 1; }

rm -rf "$OUT"; mkdir -p "$OUT"
"$CLI" test run --fixture "$FIXTURE" --no-judge --server "$TEST_SERVER" --output "$OUT" > "$OUT/run.log" 2>&1
trace=$(ls "$OUT"/*.json 2>/dev/null | head -1)
[ -n "$trace" ] || { echo "FAIL: no trace written (see $OUT/run.log)"; exit 1; }

read -r input cache_read cache_create calls <<<"$(jq -r '[.metrics.input_tokens, .metrics.cache_read_tokens, .metrics.cache_creation_tokens, .metrics.total_tool_calls] | @sh' "$trace" | tr -d "'")"
if [ "$((input + cache_read))" -eq 0 ]; then
  echo "FAIL: the provider reported no input tokens; cannot compute a ratio (trace: $trace)"; exit 1
fi
ratio=$(python3 -c "print(round($cache_read / ($input + $cache_read), 3))")
echo "fixture=$(basename "$FIXTURE") tool_calls=$calls input=$input cache_read=$cache_read cache_creation=$cache_create ratio=$ratio bar=$BAR"
python3 -c "import sys; sys.exit(0 if $ratio >= $BAR else 1)" \
  && echo "PASS: cache-read ratio $ratio >= $BAR" \
  || { echo "FAIL: cache-read ratio $ratio < $BAR — something above the cache boundary changes per iteration, or the provider reports no cache reads for this model"; exit 1; }
