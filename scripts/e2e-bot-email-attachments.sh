#!/usr/bin/env bash
# e2e-bot-email-attachments — the production check for bot email
# attachments, run once after the hub with attachments is deployed.
#
# As a harness bot (never the owner's bot or token), through the real path:
# three generated files (a PNG, a PDF and a .txt) are uploaded through
# POST /api/v1/files/upload, then ONE email is sent through
# POST /api/v1/bots/self/email with their ids, and the hub must answer that
# all three went out. Hub pod memory is printed before and after.
#
# Env:
#   NEBO_BOT_TOKEN    the harness bot's current token. Default: the token
#                     gate-server.sh carries on the runner VM,
#                     ~/harness-cache/a/token (arm A, CI Arm A). The hub
#                     rotates a bot's token on each connect, so take it fresh.
#   NEBO_API          default https://api.neboai.com
#   MAIL_TO           default alma.tuck@neboai.com
#   KUBE_CONTEXT      kubectl context for the memory readings (default: the
#                     current one). SKIP_KUBECTL=1 skips them.
#
# The bot is the one the token names (its JWT botId claim). The token is
# refused unless that bot is a harness bot. The token is never printed.
set -euo pipefail

API="${NEBO_API:-https://api.neboai.com}"
TO="${MAIL_TO:-alma.tuck@neboai.com}"
SUBJECT="Attachment test: 3 files"
# Harness bots on the "Nebo Harness" account: CI Arm A and CI Arm P (id
# prefixes). Every other bot is refused, the owner's above all.
HARNESS_BOTS="56acf1ed b3c0ad74"

die() { echo "FAIL: $*" >&2; exit 1; }
for tool in curl jq python3; do command -v "$tool" >/dev/null || die "$tool is required"; done

TOKEN="${NEBO_BOT_TOKEN:-}"
if [ -z "$TOKEN" ] && [ -s "$HOME/harness-cache/a/token" ]; then
  TOKEN="$(cat "$HOME/harness-cache/a/token")"
fi
[ -n "$TOKEN" ] || die "set NEBO_BOT_TOKEN to the harness bot's current token"

# Who the token is: the hub honours the token's botId claim, not any env var.
BOT_ID="$(python3 - "$TOKEN" <<'PY'
import base64, json, sys
parts = sys.argv[1].strip().split(".")
if len(parts) != 3:
    sys.exit("not a JWT")
claims = json.loads(base64.urlsafe_b64decode(parts[1] + "=" * (-len(parts[1]) % 4)))
print(claims.get("botId", ""))
PY
)" || die "NEBO_BOT_TOKEN is not a bot token"
[ -n "$BOT_ID" ] || die "the token names no bot (botId claim missing): not a bot token"
case " $HARNESS_BOTS " in
  *" ${BOT_ID:0:8} "*) ;;
  *) die "the token is for bot ${BOT_ID:0:8}…, which is not a harness bot. Only CI Arm A or CI Arm P may run this; never the owner's bot or token." ;;
esac
echo "bot: ${BOT_ID:0:8}… (harness)"
echo "hub: $API"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# ── The three files ────────────────────────────────────────────────────
python3 - "$work" <<'PY'
import struct, sys, zlib
out = sys.argv[1]
w, h = 96, 64
rows = b"".join(b"\x00" + bytes(v for x in range(w) for v in (x * 2 % 256, y * 4 % 256, 160)) for y in range(h))
def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open(f"{out}/attachment-test.png", "wb").write(png)

text = b"BT /F1 24 Tf 72 720 Td (Nebo attachment test) Tj ET"
objs = [
    b"<</Type/Catalog/Pages 2 0 R>>",
    b"<</Type/Pages/Kids[3 0 R]/Count 1>>",
    b"<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>",
    b"<</Length %d>>stream\n" % len(text) + text + b"\nendstream",
    b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>",
]
pdf, offsets = b"%PDF-1.4\n", []
for i, o in enumerate(objs, 1):
    offsets.append(len(pdf))
    pdf += b"%d 0 obj\n" % i + o + b"\nendobj\n"
xref = len(pdf)
pdf += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1) + b"".join(b"%010d 00000 n \n" % off for off in offsets)
pdf += b"trailer\n<</Size %d/Root 1 0 R>>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
open(f"{out}/attachment-test.pdf", "wb").write(pdf)

open(f"{out}/attachment-test.txt", "w").write("Nebo attachment test.\nThree files: a PNG, a PDF and this text.\n")
PY
ls -l "$work"

# ── Hub memory, before ─────────────────────────────────────────────────
top() {
  [ "${SKIP_KUBECTL:-}" = "1" ] && { echo "(kubectl skipped)"; return 0; }
  local ctx=(); [ -n "${KUBE_CONTEXT:-}" ] && ctx=(--context "$KUBE_CONTEXT")
  echo "── hub pods, $1 ($(date -u +%FT%TZ)) ──"
  kubectl "${ctx[@]}" top pod -n nebo -l app.kubernetes.io/name=neboloop \
    || die "kubectl top failed (set KUBE_CONTEXT, or SKIP_KUBECTL=1 to run without memory readings)"
}
top before

# ── Upload, through the one upload path ────────────────────────────────
ids=()
for f in attachment-test.png attachment-test.pdf attachment-test.txt; do
  resp="$(curl -sS -w '\n%{http_code}' -H "Authorization: Bearer $TOKEN" -F "file=@$work/$f" "$API/api/v1/files/upload")" \
    || die "upload of $f: no answer from the hub"
  code="${resp##*$'\n'}"; body="${resp%$'\n'*}"
  [ "$code" = "201" ] || [ "$code" = "200" ] || die "upload of $f: HTTP $code $body"
  id="$(jq -er '.fileId' <<<"$body")" || die "upload of $f: no fileId in $body"
  echo "uploaded $f → $id ($(jq -r '.size' <<<"$body") bytes, $(jq -r '.mimeType' <<<"$body"))"
  ids+=("$id")
done

# ── Send ONE email with the three ids ──────────────────────────────────
payload="$(jq -n --arg to "$TO" --arg subject "$SUBJECT" \
  --arg text "This message carries three attachments: attachment-test.png, attachment-test.pdf and attachment-test.txt. Each should open." \
  --args '{to: $to, subject: $subject, bodyText: $text, attachments: $ARGS.positional}' "${ids[@]}")"
resp="$(curl -sS -w '\n%{http_code}' -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d "$payload" "$API/api/v1/bots/self/email")" || die "send: no answer from the hub"
code="${resp##*$'\n'}"; body="${resp%$'\n'*}"
echo "── hub response (HTTP $code) ──"
jq . <<<"$body" 2>/dev/null || echo "$body"
[ "$code" = "200" ] || die "send: HTTP $code"
[ "$(jq -r '.ok' <<<"$body")" = "true" ] || die "send: the hub did not say ok"
attached="$(jq -r '.attachments // 0' <<<"$body")"
[ "$attached" = "3" ] || die "send: the hub confirmed $attached attachments, not 3 (a hub without attachments ignores the field)"
echo "sent to $TO from $(jq -r '.sentFrom' <<<"$body") with $attached attachments (message $(jq -r '.messageId' <<<"$body"))"

# ── Hub memory, after (metrics trail by up to a minute) ────────────────
top "right after"
if [ "${SKIP_KUBECTL:-}" != "1" ]; then
  sleep 60
  top "60 s after"
fi

echo "PASS: one email, three attachments confirmed by the hub. Check $TO for \"$SUBJECT\" and open all three files."
