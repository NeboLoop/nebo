#!/usr/bin/env bash
# gate-sandbox — run a command in one harness-gate job's own view of the VM.
#
# The gate's server, and the test runner that sets up each fixture, used to
# see the runner's real home and /tmp. So a later run could read what an
# earlier one left there: `agents-delegate` found and shared a weekly report
# a coworker had written to the VM home in another run, and a coworker
# browsed ~/harness-runs, every earlier run's transcripts (2026-09-24).
#
# Inside this view:
#   $HOME   is GATE_JOB/home, empty when the job starts
#   /tmp    is GATE_JOB/tmp, so fixture scratch (/tmp/nebo-eval/<tag>) and
#           fixed /tmp paths are this job's alone, even with another gate
#           job running on the same VM at the same time
#   cwd     is GATE_JOB/work, a fresh export of the code under test with
#           fixtures/ and suites/ left out: the model never reads a check
# and nothing else of the real home is visible except what the job itself
# runs from: its checkout and temp directory, the toolchains the proof
# fixtures build with, the claude CLI that judges, and the browsers.
#
# Both the server and `nebo-cli test run` go through here, so a fixture's
# setup writes where the model looks. Usage: gate-sandbox.sh CMD [ARGS...]
# with GATE_JOB set (the job directory the workflow made).
set -euo pipefail

: "${GATE_JOB:?set GATE_JOB to the directory of this job}"
for d in home tmp work; do
  [ -d "$GATE_JOB/$d" ] || { echo "gate-sandbox: $GATE_JOB/$d is missing" >&2; exit 1; }
done

args=(--dev-bind / / --bind "$GATE_JOB/home" "$HOME" --bind "$GATE_JOB/tmp" /tmp)
# What the job runs from, bound back at the same paths. Sources resolve in the
# real filesystem, so these are reachable even though they live under $HOME.
for keep in "${GITHUB_WORKSPACE:-}" "${RUNNER_TEMP:-}"; do
  if [ -n "$keep" ] && [ -d "$keep" ]; then args+=(--bind "$keep" "$keep"); fi
done
# cargo and rustup write their caches and locks when a proof builds.
for tool in .cargo .rustup; do
  if [ -e "$HOME/$tool" ]; then args+=(--bind "$HOME/$tool" "$HOME/$tool"); fi
done
for tool in .local/bin .local/share/claude .cache/ms-playwright; do
  if [ -e "$HOME/$tool" ]; then args+=(--ro-bind "$HOME/$tool" "$HOME/$tool"); fi
done

exec bwrap "${args[@]}" --chdir "$GATE_JOB/work" -- "$@"
