#!/usr/bin/env bash
# The engine as a macOS LaunchAgent, end to end, under a TEST identity:
# its own bundle id, label, port and Nebo folder. Never the installed Nebo
# (port 27895, ~/Library/Application Support/Nebo, /Applications/Nebo.app)
# and never `make dev`. Everything it registers is removed on exit.
#
# Builds a signed test bundle around a built `nebo` (the desktop app's
# executable), registers its agent with SMAppService through the app's own
# verb (`nebo --engine-service install`), then checks what the spec's
# macOS column asks (neboloop docs/prd/desktop-engine-service.md §15):
# register → serving; ProcessType Interactive; no sleep assertion; kill -9
# → back; a stall → exit 70 → back; Quit → stays down; kickstart → back; an
# update swapped and health-gated, a bad one rolled back; a held port → exit
# 75 → takes over; unregister → nothing left.
#
#   NEBO_BIN=target/debug/nebo scripts/test-engine-service-macos.sh
#
# Env: NEBO_BIN (required), SIGN_IDENTITY (Developer ID; SMAppService will
# not register an unsigned bundle), PORT (37895), SKIP_STALL=1 (skips the
# ~4 min stall check), SKIP_UPDATE=1 (skips the update checks, which run
# `cargo test -p nebo-updater` against the registered agent).
set -uo pipefail

NEBO_BIN=${NEBO_BIN:?set NEBO_BIN to a built nebo (the desktop executable)}
SIGN_IDENTITY=${SIGN_IDENTITY:-Developer ID Application: Alma Tuck (7Y2D3KQ2UM)}
PORT=${PORT:-37895}
LABEL=dev.neboai.nebo.engine.test
BUNDLE_ID=dev.neboai.nebo.enginetest
ROOT=${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/nebo-engine-service.XXXXXX")
APP="$WORK/NeboEngineTest.app"
HOME_DIR="$WORK/home"
KEY="engine-service-test-$$"
UID_=$(id -u)
SERVICE="gui/$UID_/$LABEL"
FAILS=0

[ "$PORT" != 27895 ] || { echo "refusing the installed Nebo's port"; exit 2; }
if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "port $PORT is in use; pick another PORT"; exit 2
fi

pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; FAILS=$((FAILS + 1)); }
verb() { NEBO_HOME="$HOME_DIR" NEBO_MCP_API_KEY="$KEY" "$APP/Contents/MacOS/nebo" --engine-service "$1" --label "$LABEL" --port "$PORT"; }
health() { curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" 2>/dev/null; }
engine_pid() { launchctl print "$SERVICE" 2>/dev/null | awk -F' = ' '/^\tpid = /{print $2; exit}'; }
last_exit() { launchctl print "$SERVICE" 2>/dev/null | awk -F' = ' '/^\tlast exit code = /{print $2; exit}'; }
quit_engine() { curl -fsS --max-time 5 -X POST -H "Authorization: Bearer $KEY" "http://127.0.0.1:$PORT/api/v1/engine/quit" >/dev/null 2>&1; }
# wait_for <seconds> <command...>: true once the command succeeds.
wait_for() {
  local secs=$1; shift
  local end=$((SECONDS + secs))
  while [ $SECONDS -lt $end ]; do "$@" && return 0; sleep 1; done
  return 1
}
serving() { health | grep -q '"role":"engine"'; }
down() { ! health >/dev/null; }

cleanup() {
  verb uninstall >/dev/null 2>&1
  launchctl bootout "$SERVICE" >/dev/null 2>&1
  [ -n "${HOLDER:-}" ] && kill "$HOLDER" 2>/dev/null
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$APP" >/dev/null 2>&1
  rm -rf "$WORK"
  if launchctl print "$SERVICE" >/dev/null 2>&1; then echo "LEFT: $SERVICE still loaded"; fi
}
trap cleanup EXIT

# ── The test bundle ─────────────────────────────────────────────────────
# (Never `nebo --version`: with no role argument the executable is the app.)
VERSION=$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Library/LaunchAgents" "$HOME_DIR"
cp "$NEBO_BIN" "$APP/Contents/MacOS/nebo"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
<key>CFBundleExecutable</key><string>nebo</string>
<key>CFBundleName</key><string>NeboEngineTest</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>CFBundleVersion</key><string>$VERSION</string>
<key>LSUIElement</key><true/>
</dict></plist>
EOF
PLIST="$APP/Contents/Library/LaunchAgents/$LABEL.plist"
cp "$ROOT/src-tauri/LaunchAgents/dev.neboai.nebo.engine.plist" "$PLIST"
PB=/usr/libexec/PlistBuddy
$PB -c "Set :Label $LABEL" "$PLIST"
$PB -c "Set :AssociatedBundleIdentifiers:0 $BUNDLE_ID" "$PLIST"
for kv in "NEBO_HOME=$HOME_DIR" "NEBO_PORT=$PORT" "NEBO_KEYRING=0" "NEBO_MCP_API_KEY=$KEY" \
  NEBOAI_API_URL=http://127.0.0.1:9 NEBOAI_JANUS_URL=http://127.0.0.1:9 \
  NEBOAI_COMMS_URL=http://127.0.0.1:9 NEBOAI_TUNNEL_URL=http://127.0.0.1:9; do
  $PB -c "Add :EnvironmentVariables:${kv%%=*} string ${kv#*=}" "$PLIST"
done
codesign --force --sign "$SIGN_IDENTITY" --identifier "$BUNDLE_ID" --options runtime \
  --entitlements "$ROOT/assets/macos/nebo.entitlements" "$APP/Contents/MacOS/nebo" >/dev/null 2>&1 \
  && codesign --force --sign "$SIGN_IDENTITY" --identifier "$BUNDLE_ID" --options runtime \
    --entitlements "$ROOT/assets/macos/nebo.entitlements" "$APP" >/dev/null 2>&1 \
  || { echo "could not sign the test bundle with '$SIGN_IDENTITY'"; exit 2; }
echo "test bundle: $APP ($BUNDLE_ID, $LABEL, port $PORT, version $VERSION)"

# ── Register → serving ──────────────────────────────────────────────────
out=$(verb install 2>&1)
if echo "$out" | grep -q '"status":"enabled"'; then pass "register: $out"; else fail "register: $out"; exit 1; fi
if wait_for 90 serving; then pass "serving: $(health)"; else fail "not serving within 90 s"; exit 1; fi
health | grep -q "\"version\":\"$VERSION\"" && pass "health reports $VERSION" || fail "health version: $(health)"
health | grep -q '"supervised":true' && pass "supervised" || fail "not supervised: $(health)"
PRINT=$(launchctl print "$SERVICE")
echo "$PRINT" | grep -q 'spawn type = interactive' && pass "ProcessType Interactive" || fail "spawn type: $(echo "$PRINT" | grep 'spawn type')"
echo "$PRINT" | grep -q 'NEBO_SUPERVISED => launchd' && pass "NEBO_SUPERVISED=launchd" || fail "env: no NEBO_SUPERVISED"
PID=$(engine_pid)
if pmset -g assertions | grep -E "pid $PID\(" | grep -qE 'PreventSystemSleep|PreventUserIdleSystemSleep'; then
  fail "the engine holds a sleep assertion"
else
  pass "no sleep assertion held by the engine (pid $PID)"
fi
[ -s "$HOME_DIR/logs/engine-stdio.log" ] || [ -f "$HOME_DIR/logs/engine-stdio.log" ] && pass "stdio to logs/engine-stdio.log" || fail "no engine-stdio.log"

# ── kill -9 → back within 15 s ──────────────────────────────────────────
kill -9 "$PID"
if wait_for 15 sh -c "[ -n \"\$(launchctl print $SERVICE | awk -F' = ' '/^\tpid = /{print \$2}')\" ] && [ \"\$(launchctl print $SERVICE | awk -F' = ' '/^\tpid = /{print \$2}')\" != $PID ]"; then
  pass "kill -9: launchd started a new engine ($(engine_pid))"
else
  fail "kill -9: no new engine within 15 s"
fi
wait_for 60 serving && pass "serving again after kill -9" || fail "not serving after kill -9"

# ── A stall → exit 70 → back ────────────────────────────────────────────
if [ "${SKIP_STALL:-}" != 1 ]; then
  echo 900 > "$HOME_DIR/TEST_STALL"
  launchctl kickstart -k "$SERVICE" >/dev/null
  STALLED=$(engine_pid)
  sleep 5
  if wait_for 300 sh -c "[ \"\$(launchctl print $SERVICE | awk -F' = ' '/^\tlast exit code = /{print \$2; exit}')\" = '70: EX_SOFTWARE' ]"; then
    pass "stall: the watchdog exited 70"
  else
    fail "stall: no exit 70 within 300 s (last exit: $(last_exit))"
  fi
  wait_for 60 serving && pass "serving again after the stall" || fail "not serving after the stall"
fi

# ── Quit → down, and stays down ─────────────────────────────────────────
quit_engine
if wait_for 15 down; then pass "Quit: the engine stopped"; else fail "Quit: still serving after 15 s"; fi
sleep 60
if down && [ -z "$(engine_pid)" ]; then pass "Quit: not started again in 60 s (last exit: $(last_exit))"; else fail "Quit: launchd started it again"; fi

# ── kickstart → back ────────────────────────────────────────────────────
verb kickstart >/dev/null
wait_for 60 serving && pass "kickstart: serving" || fail "kickstart: not serving"

# ── An app update under launchd: swap, start, health gate, rollback ─────
if [ "${SKIP_UPDATE:-}" != 1 ]; then
  # The same app, its Info.plist naming a version its engine never reports.
  NEVER="$WORK/never/NeboEngineTest.app"
  mkdir -p "$WORK/never" && cp -R "$APP" "$NEVER"
  $PB -c "Set :CFBundleShortVersionString 0.0.0-never" "$NEVER/Contents/Info.plist"
  codesign --force --sign "$SIGN_IDENTITY" --identifier "$BUNDLE_ID" --options runtime \
    --entitlements "$ROOT/assets/macos/nebo.entitlements" "$NEVER" >/dev/null 2>&1
  if (cd "$ROOT" && NEBO_TEST_APP="$APP" NEBO_TEST_APP_NEVER="$NEVER" NEBO_TEST_LABEL="$LABEL" NEBO_TEST_PORT="$PORT" NEBO_TEST_KEY="$KEY" \
      NEBO_TEST_VERSION="$VERSION" NEBO_TEST_HOME="$HOME_DIR" \
      cargo test -p nebo-updater --lib -- --ignored --exact apply::tests::macos_service_update_under_launchd) \
      > "$WORK/update-test.log" 2>&1 && grep -q 'test result: ok. 1 passed' "$WORK/update-test.log"; then
    pass "update: swapped + gated; a never-healthy and an unsigned update rolled back"
  else
    fail "update: $(tail -20 "$WORK/update-test.log")"
  fi
  wait_for 60 serving && pass "serving after the updates" || fail "not serving after the updates"
fi

# ── A held port → exit 75 → takes over once free ────────────────────────
quit_engine; wait_for 15 down
python3 -c "
import socket,time
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1',$PORT)); s.listen(1); time.sleep(600)" &
HOLDER=$!
sleep 1
launchctl kickstart "$SERVICE" >/dev/null
if wait_for 30 sh -c "launchctl print $SERVICE | grep -q 'last exit code = 75'"; then
  pass "port held: the engine exited 75"
else
  fail "port held: no exit 75 (last exit: $(last_exit))"
fi
kill "$HOLDER"; HOLDER=
wait_for 30 serving && pass "port free: the engine took it over" || fail "port free: not serving within 30 s"

# ── Unregister → nothing left ───────────────────────────────────────────
out=$(verb uninstall 2>&1)
sleep 2
if echo "$out" | grep -q '"status":"notRegistered"' && ! launchctl print "$SERVICE" >/dev/null 2>&1 && down; then
  pass "unregister: not registered, not loaded, nothing on $PORT"
else
  fail "unregister: $out / loaded: $(launchctl print "$SERVICE" >/dev/null 2>&1 && echo yes || echo no)"
fi
[ -e "$HOME/Library/LaunchAgents/$LABEL.plist" ] && fail "a plist was left in ~/Library/LaunchAgents" || pass "no file in ~/Library/LaunchAgents"

echo
[ $FAILS -eq 0 ] && echo "all checks passed" || echo "$FAILS check(s) failed"
exit $FAILS
