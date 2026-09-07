#!/usr/bin/env bash
# canary-install — schedule scripts/canary.sh daily at 06:30 on this Mac
# through launchd, as the user. Idempotent: re-running replaces the job.
#   make canary-install
#   make canary-uninstall
set -eu
REPO="$(cd "$(dirname "$0")/.." && pwd)"
LABEL="com.neboai.engine-canary"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs/Nebo"
if [ "${1:-}" = "--remove" ]; then
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"; echo "canary: schedule removed"; exit 0
fi
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>/bin/bash</string><string>$REPO/scripts/canary.sh</string></array>
  <key>StartCalendarInterval</key><dict><key>Hour</key><integer>6</integer><key>Minute</key><integer>30</integer></dict>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/Nebo/canary-launchd.log</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/Nebo/canary-launchd.log</string>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string></dict>
</dict></plist>
EOF
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "canary: scheduled daily at 06:30 ($PLIST); config in ~/.config/nebo/canary.env"
