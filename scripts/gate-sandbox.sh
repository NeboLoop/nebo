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
# setup writes where the model looks. Usage: gate-sandbox.sh [--server] CMD
# [ARGS...] with GATE_JOB set (the job directory the workflow made).
#
# Two views. The runner's (the default) keeps its checkout (the fixtures and
# suites it runs) and the job directory (traces, replay sets). The server's
# (--server) is where the model's commands run, so it keeps only what the
# server runs from: GATE_JOB/bin, its NEBO_HOME, the working directory, the
# build cache, and the source tree it was built from, read-only. A debug
# build reads its embedded folders from that tree at run time (rust-embed:
# the database migrations, the app's build), so without it the server
# migrates to nothing and stops (36309407616, "no such table"). The checks
# (every fixtures/ and suites/ in the checkout) and the rest of the job
# directory (every earlier run's server log and traces, the runner's own
# Nebo folder) are not in it: the v0.16.0 release proof found the model
# reading both (replay-thread-c52ae090, agent-spawn-explore). The source
# itself is the working directory's anyway.
set -euo pipefail

server=""
if [ "${1:-}" = "--server" ]; then
  server=1
  shift
fi
: "${GATE_JOB:?set GATE_JOB to the directory of this job}"
for d in home tmp work; do
  [ -d "$GATE_JOB/$d" ] || { echo "gate-sandbox: $GATE_JOB/$d is missing" >&2; exit 1; }
done

args=(--dev-bind / / --bind "$GATE_JOB/home" "$HOME" --bind "$GATE_JOB/tmp" /tmp)
# Nothing of the machine the VM runs on: a folder the VM host shares in
# (Lima's default template mounts the Mac's home) is covered in both views.
# A gate run's `find /` once listed the owner's Mail under /Users/stadium
# (2026-09-28); the VM mounts nothing now (harden-stadium-ci.sh --host), and
# this keeps a rebuilt VM from bringing it back into view. Rosetta's mount is
# Lima's x86 translator, not a host folder.
while read -r shared; do
  if [ -n "$shared" ] && [ "$shared" != /mnt/lima-rosetta ]; then args+=(--tmpfs "$shared"); fi
done < <(findmnt -rn -t virtiofs,9p -o TARGET 2>/dev/null || true)
# What the job runs from, bound back at the same paths. Sources resolve in the
# real filesystem, so these are reachable even though they live under $HOME.
if [ -n "$server" ]; then
  # The job directory covered, then the server's own parts bound back. The
  # checkout (this commit's, and the code under test's in under-test/) comes
  # back read-only with its checks covered. The build cache stays where
  # CARGO_TARGET_DIR says, so an employee's build in the working directory is
  # as warm as before.
  if [ -n "${RUNNER_TEMP:-}" ] && [ -d "$RUNNER_TEMP" ]; then args+=(--tmpfs "$RUNNER_TEMP"); fi
  if [ -n "${GITHUB_WORKSPACE:-}" ] && [ -d "$GITHUB_WORKSPACE" ]; then
    args+=(--ro-bind "$GITHUB_WORKSPACE" "$GITHUB_WORKSPACE")
    for tree in "$GITHUB_WORKSPACE" "$GITHUB_WORKSPACE/under-test"; do
      for checks in fixtures suites; do
        if [ -d "$tree/$checks" ]; then args+=(--tmpfs "$tree/$checks"); fi
      done
    done
  fi
  args+=(--ro-bind "$GATE_JOB/bin" "$GATE_JOB/bin" --bind "$GATE_JOB/nebo-home" "$GATE_JOB/nebo-home" --bind "$GATE_JOB/work" "$GATE_JOB/work")
  # The browser the job linked into bin lives in the runner's tool cache,
  # under the real home this view covers: bind its directory back, read-only.
  if [ -L "$GATE_JOB/bin/chromium" ]; then
    browser="$(dirname "$(readlink -f "$GATE_JOB/bin/chromium")")"
    args+=(--ro-bind "$browser" "$browser")
  fi
  if [ -n "${CARGO_TARGET_DIR:-}" ] && [ -d "$CARGO_TARGET_DIR" ]; then
    args+=(--bind "$CARGO_TARGET_DIR" "$CARGO_TARGET_DIR")
  fi
else
  for keep in "${GITHUB_WORKSPACE:-}" "${RUNNER_TEMP:-}"; do
    if [ -n "$keep" ] && [ -d "$keep" ]; then args+=(--bind "$keep" "$keep"); fi
  done
fi
# cargo and rustup write their caches and locks when a proof builds.
for tool in .cargo .rustup; do
  if [ -e "$HOME/$tool" ]; then args+=(--bind "$HOME/$tool" "$HOME/$tool"); fi
done
for tool in .local/bin .local/share/claude .cache/ms-playwright; do
  if [ -e "$HOME/$tool" ]; then args+=(--ro-bind "$HOME/$tool" "$HOME/$tool"); fi
done

exec bwrap "${args[@]}" --chdir "$GATE_JOB/work" -- "$@"
