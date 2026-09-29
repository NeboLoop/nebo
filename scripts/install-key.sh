# install-key — sourced by the dev scripts that call a running Nebo's local
# API. Every caller proves itself to that API; a script is the owner's own
# client, so it carries the install key, as the first segment of the server
# address it joins its paths onto (`host:port/k/<key>`).
#
#   . "$(dirname "$0")/install-key.sh"
#   TEST_SERVER="$(with_install_key "$TEST_SERVER")"
#
# The key: NEBO_MCP_API_KEY when set, else the one the server made in its
# folder (NEBO_HOME, or the platform's default).
nebo_install_key() {
  if [ -n "${NEBO_MCP_API_KEY:-}" ]; then printf '%s' "$NEBO_MCP_API_KEY"; return; fi
  local home="${NEBO_HOME:-}"
  if [ -z "$home" ]; then
    case "$(uname -s)" in
      Darwin) home="$HOME/Library/Application Support/Nebo" ;;
      *) home="${XDG_DATA_HOME:-$HOME/.local/share}/nebo" ;;
    esac
  fi
  tr -d '[:space:]' < "$home/.install-key" 2>/dev/null
}

with_install_key() {
  local key; key="$(nebo_install_key)"
  [ -n "$key" ] || { echo "FAIL: no install key (start Nebo once, or set NEBO_MCP_API_KEY)." >&2; exit 1; }
  printf '%s/k/%s' "$1" "$key"
}
