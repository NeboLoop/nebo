#!/usr/bin/env bash
#
# sign-probe.sh — verify Azure Trusted Signing is actually working before a release.
#
# The Trusted Signing *data plane* can be silently blocked (e.g. after a lapsed Azure
# bill) while every control-plane status page still reads "healthy". The ONLY reliable
# check is a real :sign call. This temporarily grants the signed-in user the signer role,
# signs a throwaway SHA-256 digest, reads the result, and revokes the role.
#
# Usage:  bash scripts/sign-probe.sh
# Exit:   0 = signing works (Succeeded)   |   1 = still blocked / error
# Requires: az (logged in as a subscription Owner), openssl, python3, curl.
#
# See docs/sme/RELEASE.md → "Azure Trusted Signing Billing Hold".
set -uo pipefail

RG="${NEBO_SIGN_RG:-nebo}"
ACCOUNT="${NEBO_SIGN_ACCOUNT:-nebosigning}"
PROFILE="${NEBO_SIGN_PROFILE:-neboloop-public}"
ENDPOINT="${NEBO_SIGN_ENDPOINT:-https://eus.codesigning.azure.net}"
API="api-version=2023-06-15-preview"
ROLE="Artifact Signing Certificate Profile Signer"

SUB=$(az account show --query id -o tsv 2>/dev/null) || { echo "not logged in — run 'az login'"; exit 1; }
OID=$(az ad signed-in-user show --query id -o tsv 2>/dev/null)
SCOPE="/subscriptions/${SUB}/resourceGroups/${RG}/providers/Microsoft.CodeSigning/codeSigningAccounts/${ACCOUNT}"
BASE="${ENDPOINT}/codesigningaccounts/${ACCOUNT}/certificateprofiles/${PROFILE}"

echo "probe: sub=${SUB} account=${ACCOUNT} profile=${PROFILE}"
az role assignment create --assignee-object-id "$OID" --assignee-principal-type User --role "$ROLE" --scope "$SCOPE" -o none 2>/dev/null
cleanup() { az role assignment delete --assignee "$OID" --role "$ROLE" --scope "$SCOPE" -o none 2>/dev/null; }
trap cleanup EXIT
sleep 25   # RBAC propagation

DIGEST=$(printf 'nebo-sign-probe' | openssl dgst -sha256 -binary | base64)
TOKEN=$(az account get-access-token --scope "https://codesigning.azure.net/.default" --query accessToken -o tsv 2>/dev/null)
ID=$(curl -s -X POST "${BASE}:sign?${API}" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
     -d "{\"signatureAlgorithm\":\"RS256\",\"digest\":\"${DIGEST}\"}" \
     | python3 -c "import sys,json;d=json.load(sys.stdin);print(d.get('id') or d.get('result',{}).get('operationId',''))" 2>/dev/null)

if [ -z "$ID" ]; then echo "RESULT: ERROR — sign request not accepted (check role/login)"; exit 1; fi

ST=""
for _ in $(seq 1 8); do
  sleep 2
  ST=$(curl -s "${BASE}/sign/${ID}?${API}" -H "Authorization: Bearer $TOKEN" \
       | python3 -c "import sys,json;d=json.load(sys.stdin);print(d.get('status') or d.get('result',{}).get('status',''))" 2>/dev/null)
  { [ "$ST" = "Succeeded" ] || [ "$ST" = "Failed" ]; } && break
done

if [ "$ST" = "Succeeded" ]; then
  echo "RESULT: ✅ SUCCEEDED — signing works (op $ID)"; exit 0
else
  echo "RESULT: ❌ ${ST:-timeout} — Trusted Signing is blocked (op $ID). See RELEASE.md → Billing Hold."; exit 1
fi
