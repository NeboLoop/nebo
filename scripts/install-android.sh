#!/data/data/com.termux/files/usr/bin/bash
# Nebo on Android via Termux.
#
# Run inside Termux (https://termux.dev — install from F-Droid or GitHub,
# NOT the abandoned Play Store build):
#
#   curl -fsSL https://raw.githubusercontent.com/NeboLoop/nebo/main/scripts/install-android.sh | bash
#
# What it does: installs a minimal Ubuntu userland via proot-distro (no root
# required), downloads the official nebo-linux-arm64-headless release into it,
# and adds a `nebo` command to Termux. The nebo binary needs glibc >= 2.39,
# which is why this uses Ubuntu 24.04 rather than Debian.
set -euo pipefail

case "$(uname -m)" in
  aarch64) ;;
  *) echo "Nebo on Android needs a 64-bit ARM device (uname -m must be aarch64, got: $(uname -m))." >&2; exit 1 ;;
esac

echo "==> Installing proot-distro..."
pkg install -y proot-distro >/dev/null

echo "==> Installing Ubuntu userland (one-time, ~100MB)..."
proot-distro install ubuntu >/dev/null 2>&1 || true  # already-installed is fine

echo "==> Installing Nebo inside Ubuntu..."
proot-distro login ubuntu -- bash -c '
  set -euo pipefail
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates libwayland-client0 libopenblas0 >/dev/null
  VER=$(curl -fsSL https://cdn.neboai.com/releases/version.json | grep -o "\"version\": *\"[^\"]*\"" | head -1 | cut -d"\"" -f4)
  echo "    latest release: ${VER}"
  curl -fL --progress-bar -o /usr/local/bin/nebo "https://cdn.neboai.com/releases/${VER}/nebo-linux-arm64-headless"
  chmod +x /usr/local/bin/nebo
'

echo "==> Adding the nebo command to Termux..."
cat > "$PREFIX/bin/nebo" <<'LAUNCHER'
#!/data/data/com.termux/files/usr/bin/bash
# Keep the CPU awake while Nebo runs (needs the Termux:API notification
# permission the first time); released when Nebo exits.
command -v termux-wake-lock >/dev/null && termux-wake-lock
trap 'command -v termux-wake-unlock >/dev/null && termux-wake-unlock' EXIT
exec proot-distro login ubuntu -- /usr/local/bin/nebo --headless "$@"
LAUNCHER
chmod +x "$PREFIX/bin/nebo"

echo
echo "Done. Start your AI workforce with:  nebo"
echo "Then open  http://localhost:27895  in Chrome on this phone."
echo
echo "Tip: Android pauses background apps aggressively. For a workforce that"
echo "never clocks out, disable battery optimization for Termux (Settings →"
echo "Apps → Termux → Battery → Unrestricted) — or pair this phone with a"
echo "cloud Nebo from https://neboai.com."
