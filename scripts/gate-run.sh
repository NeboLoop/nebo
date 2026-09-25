#!/usr/bin/env bash
# gate-run — the one way a harness-gate job runs a suite, a fixture or a
# replay set: every run of every fixture on a fresh server.
#
#   gate-run.sh ENTRY RUNS OUTPUT RUNNER...
#
# ENTRY   suites/<x>.yaml, a fixture path, or replays/<set>/suite.yaml
# RUNS    runs per fixture (numbered run-1 .. run-RUNS on disk)
# OUTPUT  the trace directory
# RUNNER  the test runner and its fixed arguments (judge, model), e.g.
#         scripts/gate-sandbox.sh "$GATE_JOB/bin/nebo-runner" test run --no-judge --model m
#
# Before each run, `gate-server.sh fresh` stops the server, empties the job's
# NEBO_HOME, home, /tmp and working directory, and starts it again, so no run
# sees what an earlier run left: its memory, its files, its sessions. Runs of
# one fixture on one server used to share the bot's memory: 2026-09-25,
# helper-finishes-while-owner-chats run 2 quoted the access code run 1's
# memory extraction had stored, with no note ever read. Replays have run this
# way since #292; every fixture run does now.
#
# The bot's credentials (A_/P_ BOT_ID, BOT_TOKEN, BOOT_TOKEN, GATE_ARM,
# GATE_JOB, GATE_PORT) are for gate-server.sh; the runner never sees them.
# A run that fails does not stop the next: each run is on its own server.
# Exits non-zero when any run failed.
set -euo pipefail

[ $# -ge 4 ] || { echo "usage: gate-run.sh ENTRY RUNS OUTPUT RUNNER..." >&2; exit 2; }
entry=$1 runs=$2 output=$3
shift 3
: "${GATE_JOB:?set GATE_JOB to the directory of this job}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The fixtures of ENTRY, one absolute path per line. A replay set lives on
# the VM (~/harness-replays/<set>, put there by scripts/export-threads.py
# push), never in git, and is copied into this job's own directory so no
# other run can read it.
case "$entry" in
  replays/*/suite.yaml)
    set_name=$(basename "$(dirname "$entry")")
    [ -d "$HOME/harness-replays/$set_name" ] || { echo "no replay set $set_name on this VM"; exit 1; }
    mkdir -p "$GATE_JOB/replays"
    cp -R "$HOME/harness-replays/$set_name" "$GATE_JOB/replays/"
    suite="$GATE_JOB/replays/$set_name/suite.yaml" ;;
  suites/*) suite="$(cd "$(dirname "$entry")" && pwd)/$(basename "$entry")" ;;
  *) suite="" ;;
esac
if [ -n "$suite" ]; then
  # A script-backed suite (suites/engine-live.yaml) is not a list of
  # fixtures, and the gate does not run it.
  mapfile -t fixtures < <(python3 -c '
import os, sys, yaml
path = sys.argv[1]
suite = yaml.safe_load(open(path))
if suite.get("scripts"):
    sys.exit(f"{path} is script-backed; the gate runs fixtures only")
for f in suite.get("fixtures") or []:
    print(os.path.normpath(os.path.join(os.path.dirname(path), f)))
' "$suite")
else
  fixtures=("$(cd "$(dirname "$entry")" && pwd)/$(basename "$entry")")
fi
[ "${#fixtures[@]}" -gt 0 ] || { echo "$entry lists no fixtures"; exit 1; }

failed=""
for fixture in "${fixtures[@]}"; do
  for run in $(seq 1 "$runs"); do
    "$here/gate-server.sh" fresh
    env -u A_BOT_ID -u A_BOT_TOKEN -u A_BOOT_TOKEN -u P_BOT_ID -u P_BOT_TOKEN -u P_BOOT_TOKEN \
      "$@" --fixture "$fixture" --runs 1 --first-run "$run" \
      --server "localhost:$(cat "$GATE_JOB/server.port")" --output "$output" \
      || failed="$failed $(basename "$fixture" .yaml):run-$run"
  done
done
[ -z "$failed" ] || { echo "failed:$failed"; exit 1; }
