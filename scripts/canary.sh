#!/usr/bin/env bash
# canary — the engine's real-model check, on a schedule and before a deploy.
#
# Runs the deterministic proof (no model), then the live half against the
# Nebo on TEST_SERVER with a real model, writing to the address in
# ENGINE_CONTACT. Every model-side defect so far came from a real turn, not
# a unit test, so this is what stands between a build and a customer.
#
# Where it writes:
#   ~/Library/Logs/Nebo/canary-<date>.log     the full run
#   ~/.config/nebo/canary.last                one line: <utc> <commit> PASS|FAIL
#
# Configuration: ~/.config/nebo/canary.env (sourced if present), e.g.
#   ENGINE_CONTACT=you@example.com
#   TEST_SERVER=localhost:27895
#   NEBO_REPO=/path/to/the/checkout        (default: this script's repo)
#
#   make canary                 # run now
#   make canary-install         # run daily at 06:30 (launchd, this Mac)
#   make deploy-check           # refuse a deploy without a fresh PASS on HEAD
set -u
CONF="$HOME/.config/nebo/canary.env"
[ -f "$CONF" ] && . "$CONF"
REPO="${NEBO_REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
LOGDIR="$HOME/Library/Logs/Nebo"; mkdir -p "$LOGDIR" "$HOME/.config/nebo"
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
LOG="$LOGDIR/canary-$STAMP.log"
LAST="$HOME/.config/nebo/canary.last"
cd "$REPO" || exit 1
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)

record() { echo "$STAMP $COMMIT $1" | tee -a "$LOG" > "$LAST"; echo "canary: $1 — $LOG"; }

{
  echo "canary $STAMP on $COMMIT ($REPO)"
  echo "== proof (deterministic)"
  if ! CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target-check}" cargo test -p nebo-server -- engine:: 2>&1 | tail -20; then
    echo "proof failed"; exit 2
  fi
  echo "== live (real model)"
  ENGINE_CONTACT="${ENGINE_CONTACT:-}" TEST_SERVER="${TEST_SERVER:-localhost:27895}" bash scripts/test-engine.sh
} >> "$LOG" 2>&1
rc=$?
if [ "$rc" = 0 ]; then record PASS; exit 0; else record "FAIL($rc)"; exit 1; fi
