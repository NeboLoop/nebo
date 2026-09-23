#!/usr/bin/env bash
# Harden the Linux runner VM on the house Mac mini (Lima VM "ci" on stadium).
# Idempotent: re-run after adding a runner or rebuilding the VM.
#
#   ssh stadium '/opt/homebrew/bin/limactl shell ci -- sudo bash -s' < scripts/harden-stadium-ci.sh
#
# Why (2026-09-22): the VM filled its disk (116 GB of Go build cache, nothing
# ever pruned) and later OOM-killed both NeboLoop runner listeners while two
# emulated image builds ran beside the payrollhub runners. The units had no
# restart policy, so the runners stayed dead and jobs sat "queued" with no
# signal. This script makes each of those failures self-healing:
#   - every runner unit restarts itself;
#   - each org's runners live in their own memory slice, so one org's jobs
#     cannot OOM the other's or the OS;
#   - a daily prune keeps the disk under a ceiling.
set -euo pipefail

RUNNER_USER=stadium
HOME_DIR=/home/stadium.guest

# Per-org memory ceilings (VM has 12 GB; ~1 GB left for the OS + docker).
declare -A SLICE_MAX=([neboloop]=7G [payrollhub]=4G)
declare -A SLICE_HIGH=([neboloop]=6500M [payrollhub]=3600M)

for org in "${!SLICE_MAX[@]}"; do
  cat > "/etc/systemd/system/ci-${org}.slice" <<SLICE
[Unit]
Description=GitHub Actions runners for ${org}

[Slice]
MemoryAccounting=yes
MemoryHigh=${SLICE_HIGH[$org]}
MemoryMax=${SLICE_MAX[$org]}
SLICE
done

for unit in /etc/systemd/system/actions.runner.*.service; do
  name=$(basename "$unit")
  case "$name" in
    *NeboLoop*)   slice=ci-neboloop.slice ;;
    *payrollhub*) slice=ci-payrollhub.slice ;;
    *) echo "skip $name (no slice mapped)"; continue ;;
  esac
  mkdir -p "/etc/systemd/system/${name}.d"
  cat > "/etc/systemd/system/${name}.d/10-harden.conf" <<DROPIN
[Unit]
StartLimitIntervalSec=0

[Service]
Slice=${slice}
Restart=always
RestartSec=15
DROPIN
done

cat > /usr/local/sbin/ci-prune <<'PRUNE'
#!/usr/bin/env bash
# Keep the runner VM's disk under a ceiling. Caches only — never workspaces
# of a running job, never named volumes of running containers.
set -u
used() { df --output=pcent / | tail -1 | tr -dc 0-9; }
echo "disk before: $(used)%"
docker builder prune -f --keep-storage 30GB >/dev/null 2>&1 || true
docker image prune -af --filter "until=72h" >/dev/null 2>&1 || true
docker volume prune -f >/dev/null 2>&1 || true
# Go's build cache never shrinks on its own: trim it whenever it passes 20 GB,
# and always when the disk is past 75%.
gocache=/home/stadium.guest/.cache/go-build
if [ -d "$gocache" ]; then
  gb=$(du -s --block-size=1G "$gocache" | cut -f1)
  if [ "$gb" -gt 20 ] || [ "$(used)" -gt 75 ]; then rm -rf "$gocache"; fi
fi
echo "disk after: $(used)%"
PRUNE
chmod 755 /usr/local/sbin/ci-prune

cat > /etc/systemd/system/ci-prune.service <<'UNIT'
[Unit]
Description=Prune CI caches on the runner VM

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/ci-prune
UNIT
cat > /etc/systemd/system/ci-prune.timer <<'UNIT'
[Unit]
Description=Prune CI caches every 6 hours

[Timer]
OnBootSec=10min
OnUnitActiveSec=6h
Persistent=true

[Install]
WantedBy=timers.target
UNIT

systemctl daemon-reload
systemctl enable --now ci-prune.timer >/dev/null
# Never kill a job in flight: a runner whose directory has a live
# Runner.Worker keeps running and joins its slice on its next restart.
# Every other runner is (re)started now.
for unit in /etc/systemd/system/actions.runner.*.service; do
  name=$(basename "$unit")
  dir=$(systemctl show -p WorkingDirectory --value "$name")
  if pgrep -f "${dir}/bin[^ ]*/Runner.Worker" >/dev/null; then
    echo "busy, not restarted now: $name"
    continue
  fi
  systemctl reset-failed "$name" 2>/dev/null || true
  systemctl restart "$name"
done
systemctl start ci-prune.service
sleep 5
for unit in /etc/systemd/system/actions.runner.*.service; do
  name=$(basename "$unit")
  printf '%-70s %s %s\n' "$name" "$(systemctl is-active "$name")" "$(systemctl show -p Slice --value "$name")"
done
systemctl show -p MemoryMax --value ci-neboloop.slice ci-payrollhub.slice
