#!/usr/bin/env bash
# gate-server — the one way a harness-gate job starts and stops its server.
#
#   gate-server.sh start   boot the server under test in the job's sandbox and
#                          wait until it holds the CI bot and answers /health
#   gate-server.sh stop    stop it and let it hand the bot's lease back
#   gate-server.sh fresh   stop, empty the job's home, /tmp, NEBO_HOME and
#                          working directory, and start again
#
# `fresh` is what a replay run gets: replays in one job used to share one
# NEBO_HOME, so a later thread recalled an earlier one's memory (2026-09-25:
# 89f2b73b called an SEO keyword "a completely different vertical from solar";
# solar came from 30f9c07c, replayed before it). Every run of a replay now
# starts from the empty bot a new gate job starts from, on both arms.
#
# Env: GATE_JOB (the job directory), GATE_PORT, GATE_ARM (a or p),
# LEASE_WAIT_MINUTES, and the arm's bot as A_BOT_ID, A_BOT_TOKEN,
# A_BOOT_TOKEN or P_BOT_ID, P_BOT_TOKEN, P_BOOT_TOKEN. The server's pid and
# port are kept in GATE_JOB/server.pid and GATE_JOB/server.port (and the port
# in GITHUB_ENV, for the steps after this one); each boot logs to its own
# GATE_JOB/server-logs/<n>.log.
set -euo pipefail

: "${GATE_JOB:?set GATE_JOB to the directory of this job}"
: "${GATE_PORT:?set GATE_PORT}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

start() {
  # Each arm's bot by name; this picks one and never falls back to the other
  # (an unset arm-P secret must fail, not run as A).
  local p
  case "${GATE_ARM:-}" in
    a) p=""; NEBO_BOT_ID="${A_BOT_ID:-}" NEBO_BOT_TOKEN="${A_BOT_TOKEN:-}" NEBO_BOOT_TOKEN="${A_BOOT_TOKEN:-}" ;;
    p) p="P_"; NEBO_BOT_ID="${P_BOT_ID:-}" NEBO_BOT_TOKEN="${P_BOT_TOKEN:-}" NEBO_BOOT_TOKEN="${P_BOOT_TOKEN:-}" ;;
    *) echo "arm must be a or p, not '${GATE_ARM:-}'"; exit 1 ;;
  esac
  unset A_BOT_ID A_BOT_TOKEN A_BOOT_TOKEN P_BOT_ID P_BOT_TOKEN P_BOOT_TOKEN
  test -n "$NEBO_BOT_ID" && test -n "$NEBO_BOT_TOKEN" && test -n "$NEBO_BOOT_TOKEN" \
    || { echo "HARNESS_${p}NEBO_BOT_ID, HARNESS_${p}NEBO_BOT_TOKEN and HARNESS_${p}NEBO_BOOT_TOKEN must be set"; exit 1; }
  export NEBO_BOT_ID NEBO_BOT_TOKEN NEBO_BOOT_TOKEN
  # The run record the side-by-side report reads, written by the job's first
  # boot (never printed: the bot id is a secret; it stays on the VM with the
  # traces).
  [ -f "$GATE_JOB/run.json" ] || jq -n --arg arm "$GATE_ARM" --arg bot "$NEBO_BOT_ID" --arg code "${GATE_CODE_SHA:-}" \
    --arg ref "${GATE_REF:-}" --arg runner "${GITHUB_SHA:-}" --arg suites "${GATE_SUITES:-}" \
    --arg judge "${GATE_JUDGE:-}" --arg model "${GATE_MODEL:-}" --arg started "$(date -u +%FT%TZ)" \
    '{arm: $arm, bot_id: $bot, code_sha: $code, ref: $ref, runner_sha: $runner, suites: $suites,
      judge: ($judge == "true"), model: $model, started_at: $started}' > "$GATE_JOB/run.json"

  # A fresh NEBO_HOME so nothing on the runner leaks in. NEBO_SERVER_MODE
  # marks setup complete on boot (there is no desktop to click through);
  # NEBO_BOT_ID and NEBO_BOT_TOKEN seed the NeboAI profile with Janus
  # enabled, the same first-boot path a cloud pod takes; NEBO_BOOT_TOKEN is
  # what the hub accepts once that token has gone stale.
  export NEBO_HOME="$GATE_JOB/nebo-home" NEBO_SERVER_MODE=1
  # No company Memory on the gate: the shared KB at kb.neboai.com is keyed on
  # the bot, so every earlier gate run leaked into each fixture (2026-09-23:
  # "you declined the X card" on a fixture's FIRST run, and one run in three
  # answered from memory with no tool call). An empty URL means the server
  # wires no Memory integration; recall is the fresh NEBO_HOME's alone.
  export NEBOAI_MEMORY_URL="" RUST_LOG="info,nebo_agent=debug"

  local port="$GATE_PORT"
  # A job cancelled mid-suite leaves its server on this runner's port (the
  # runner's orphan sweep does not always reach a nohup'd child):
  # 35582848373 died on "port 27891 is already in use" behind the cancelled
  # 35582523046. Whatever listens there is a leftover of this runner's own
  # earlier job; free the port before starting.
  fuser -k -TERM "$port/tcp" 2>/dev/null && sleep 2 || true
  # A listener the job cannot kill (35585471453: port held with no owning
  # pid visible even to root) must not block the job: step past it. The port
  # is this job's for the rest of the run.
  while ss -ltn 2>/dev/null | grep -q ":$port "; do
    echo "port $port is held by something this job cannot stop; trying the next"
    port=$((port + 10))
  done
  echo "$port" > "$GATE_JOB/server.port"
  [ -z "${GITHUB_ENV:-}" ] || echo "GATE_PORT=$port" >> "$GITHUB_ENV"
  export NEBO_PORT="$port"

  mkdir -p "$GATE_JOB/server-logs"
  local log
  log="$GATE_JOB/server-logs/$(printf '%03d' "$(find "$GATE_JOB/server-logs" -name '*.log' | wc -l)").log"
  nohup "$here/gate-sandbox.sh" "$GATE_JOB/bin/nebo-server" agent > "$log" 2>&1 &
  local server=$!
  echo "$server" > "$GATE_JOB/server.pid"
  # Every gate job is the same CI bot, and the bot has one lease. While
  # another server holds it, ours logs `waiting` every 15 s from
  # backup_ship::until_released and does not listen yet; once the lease is
  # free it logs one of `past_lease` and boots. The lease wait is bounded by
  # LEASE_WAIT_MINUTES; the 120 s boot budget only counts once the lease is
  # ours (36017752499 died at 120 s behind the push gate 36017093512).
  local waiting="another copy of this bot still holds it"
  local past_lease="bot state restored from NeboAI|no committed state: starting as a new bot"
  local lease_deadline=$((SECONDS + ${LEASE_WAIT_MINUTES:-45} * 60))
  local boot_start=$SECONDS said=""
  until curl -sf -m 2 "http://localhost:$port/health" >/dev/null; do
    kill -0 "$server" 2>/dev/null || break
    if grep -q "$waiting" "$log" && ! grep -Eq "$past_lease" "$log"; then
      [ -n "$said" ] || { echo "another gate job holds the CI bot; waiting up to ${LEASE_WAIT_MINUTES:-45} min for it to hand the bot back"; said=1; }
      if [ "$SECONDS" -ge "$lease_deadline" ]; then
        echo "the CI bot was not handed back within ${LEASE_WAIT_MINUTES:-45} min; the server log:"; cat "$log"; exit 1
      fi
      boot_start=$SECONDS
    elif [ $((SECONDS - boot_start)) -ge 120 ]; then
      echo "the server did not answer /health within 120 s of holding the bot; its log:"; cat "$log"; exit 1
    fi
    sleep 2
  done
  [ -z "$said" ] || echo "the CI bot is ours after $((SECONDS / 60)) min"
  # The health probe alone proved nothing: with two jobs on one VM it
  # answered from the OTHER job's server while ours had died on "port already
  # in use", and the fixtures ran against a build that was not this commit
  # (every gate run on 2026-09-20 but one).
  if ! kill -0 "$server" 2>/dev/null; then
    echo "this job's server is not running; its log:"; cat "$log"; exit 1
  fi
  curl -sf -m 2 "http://localhost:$port/health"
  echo
  # The agent runs on Janus, the backend customers get.
  curl -sf "http://localhost:$port/api/v1/providers" \
    | jq -e '.profiles[] | select(.provider == "neboai" and .isActive)' >/dev/null \
    || { echo "no active NeboAI profile; the bot token did not seed"; exit 1; }
}

stop() {
  local server port
  server="$(cat "$GATE_JOB/server.pid" 2>/dev/null || true)"
  port="$(cat "$GATE_JOB/server.port" 2>/dev/null || echo "$GATE_PORT")"
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
  fuser -k -TERM "$port/tcp" 2>/dev/null || true
  # A server asked to exit hands the bot's lease back on its way out; let it,
  # so the next server (this job's, or a gate job waiting for the bot)
  # starts now rather than when the lease lapses after the runner reaps it.
  for _ in $(seq 1 30); do
    [ -n "$server" ] && kill -0 "$server" 2>/dev/null || break
    sleep 2
  done
  rm -f "$GATE_JOB/server.pid"
}

fresh() {
  stop
  # Everything the last server and the runs against it could write: the bot's
  # database and files, $HOME, /tmp and the working directory (restored from
  # the export of the code under test).
  local d
  for d in nebo-home home tmp work; do
    rm -rf "${GATE_JOB:?}/$d"
    mkdir -p "$GATE_JOB/$d"
  done
  tar -x -C "$GATE_JOB/work" -f "$GATE_JOB/work.tar"
  start
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  fresh) fresh ;;
  *) echo "usage: gate-server.sh start|stop|fresh" >&2; exit 2 ;;
esac
