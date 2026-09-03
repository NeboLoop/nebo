#!/usr/bin/env bash
# test-repo — layer L4 of docs/plans/2026-09-02-parity-test-plan.md: the PRD
# one-sentence test and the seven-way fan-out on real open-source repos.
#
# Each fixture copies a warmed probe clone under /tmp/nebo-test (see
# fixtures/repo/*.yaml; probes live in /tmp/nebo-test/probe and are made once
# with scripts/test-repo-probes.sh), runs ALONE against the live server with a
# wall-clock guard, and is then verified here with the project's own tests
# and git, not with the judge. Exits non-zero on the first failure.
#
#   make test-repo                       # all three
#   FIXTURE=repo-fd-planted-bug make test-repo
#   RUNS=3 make test-repo                # the on-par gate
set -u
TEST_SERVER="${TEST_SERVER:-localhost:27895}"
CLI="${NEBO_CLI:-./target/debug/nebo-cli}"
OUT="${OUT:-/tmp/nebo-traces-repo}"
GUARD_SECS="${GUARD_SECS:-900}"
ONLY="${FIXTURE:-}"
GRADER="${GRADER:-claude-sonnet-4-6}"
RUNS="${RUNS:-1}"   # the on-par gate wants 3/3; each run re-plants the bug in setup, verification checks the last

curl -sf -m 3 "http://$TEST_SERVER/health" >/dev/null || { echo "FAIL: no Nebo on $TEST_SERVER — start one with 'make dev' first."; exit 1; }
[ -x "$CLI" ] || { echo "FAIL: $CLI not built (cargo build -p nebo-cli)."; exit 1; }
for p in fd ripgrep sveltestrap; do [ -d /tmp/nebo-test/probe/$p ] || { echo "FAIL: probe /tmp/nebo-test/probe/$p missing — run scripts/test-repo-probes.sh"; exit 1; }; done
mkdir -p "$OUT"

run_fixture() { # id
  local id="$1"
  echo "== $id"
  rm -rf "$OUT/$id"
  perl -e "alarm $GUARD_SECS; exec @ARGV" "$CLI" test run --fixture "fixtures/repo/$id.yaml" --server "$TEST_SERVER" \
    --output "$OUT/$id" --grader "$GRADER" --runs "$RUNS" > "$OUT/$id.log" 2>&1
  local rc=$?
  grep -E "✓|✗|verified:|Avg tokens" "$OUT/$id.log" | sed 's/^/   /'
  [ "$rc" = 0 ] || { echo "FAIL [$id]: fixture run exited $rc (see $OUT/$id.log)"; exit 1; }
}
verify_test() { # dir, cargo test args…
  local dir="$1"; shift
  (cd "$dir" && cargo test -q "$@" > "$OUT/verify-$(basename "$dir").log" 2>&1) \
    && echo "   verified: $(basename "$dir") test passes" \
    || { echo "FAIL [$(basename "$dir")]: the project's test still fails after the fix (see $OUT/verify-$(basename "$dir").log)"; exit 1; }
}

if [ -z "$ONLY" ] || [ "$ONLY" = repo-fd-planted-bug ]; then
  run_fixture repo-fd-planted-bug
  verify_test /tmp/nebo-test/fd test_size
  changed=$(git -C /tmp/nebo-test/fd status --porcelain | wc -l | tr -d ' ')
  [ "$changed" = 1 ] || { echo "FAIL [fd]: expected exactly 1 changed file, got $changed"; git -C /tmp/nebo-test/fd status --porcelain; exit 1; }
fi

if [ -z "$ONLY" ] || [ "$ONLY" = repo-ripgrep-8k-file ]; then
  run_fixture repo-ripgrep-8k-file
  verify_test /tmp/nebo-test/ripgrep --bin rg flags::defs::test_max_count
  changed=$(git -C /tmp/nebo-test/ripgrep status --porcelain | wc -l | tr -d ' ')
  [ "$changed" = 1 ] || { echo "FAIL [ripgrep]: expected exactly 1 changed file, got $changed"; git -C /tmp/nebo-test/ripgrep status --porcelain; exit 1; }
fi

if [ -z "$ONLY" ] || [ "$ONLY" = repo-sveltestrap-fanout ]; then
  run_fixture repo-sveltestrap-fanout
  n=0
  for c in Alert Badge Button Card Col Container Dropdown; do
    grep -q "nb-restyled" "/tmp/nebo-test/sveltestrap/src/$c/$c.svelte" && n=$((n+1)) || echo "   missing: $c"
  done
  [ "$n" = 7 ] || { echo "FAIL [sveltestrap]: $n of 7 components restyled"; exit 1; }
  conflicts=$(find /tmp/nebo-test/sveltestrap -name '*.nebo-conflict' | wc -l | tr -d ' ')
  [ "$conflicts" = 0 ] || { echo "FAIL [sveltestrap]: $conflicts conflict files"; exit 1; }
  # A git-repo merge lands as commits on the owner's branch (one per worker), so
  # the change set is measured against the probe's HEAD, and the tree must be clean.
  base=$(git -C /tmp/nebo-test/probe/sveltestrap rev-parse HEAD)
  changed=$(git -C /tmp/nebo-test/sveltestrap diff --name-only "$base" HEAD | wc -l | tr -d ' ')
  dirty=$(git -C /tmp/nebo-test/sveltestrap status --porcelain | wc -l | tr -d ' ')
  [ "$changed" = 7 ] || { echo "FAIL [sveltestrap]: expected exactly 7 files changed since $base, got $changed"; git -C /tmp/nebo-test/sveltestrap diff --name-only "$base" HEAD; exit 1; }
  [ "$dirty" = 0 ] || { echo "FAIL [sveltestrap]: merge left the tree dirty ($dirty paths)"; git -C /tmp/nebo-test/sveltestrap status --porcelain; exit 1; }
  echo "   verified: 7/7 restyled, 0 conflicts, 7 files changed as merge commits, tree clean"
fi
echo
echo "test-repo: all green"
