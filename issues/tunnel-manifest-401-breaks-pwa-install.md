# PWA Install Broken for Every Tunneled Bot — `manifest.webmanifest` 401s

**Repos:** `nebo` (fix lives here) + `neboloop` (where the 401 is returned)
**Found:** 2026-07-28, live against `neboai.com/t/<botID>/` with a local Mac bot
**Priority:** Medium — no data or security impact, but Add-to-Home-Screen is dead on the exact
surface the tunnel exists to serve (phones), and it fails silently.

## Issue

Loading a bot through the reverse tunnel returns **401 for `manifest.webmanifest`**, so the browser
discards the web app manifest. Add-to-Home-Screen / Install App does not work for any tunneled bot —
desktop or mobile, cloud or local.

The intent is already in the code. `internal/api/tunnel.go:289-292` rewrites the manifest and
apple-touch-icon hrefs with the comment *"so Add-to-Home-Screen works per bot."* The paths are right.
The fetch is unauthenticated, so it never gets that far.

## Reproduction

1. Open `https://neboai.com/manage`, pick any bot, click **Open**.
2. Observe the console:

```
[ERROR] Failed to load resource: the server responded with a status of 401 ()
        @ https://neboai.com/t/28b0ccd5-.../manifest.webmanifest
[ERROR] Manifest fetch from https://neboai.com/t/28b0ccd5-.../manifest.webmanifest
        failed, code 401
```

Fires twice per load. Everything else on the page is clean — WS upgrades, REST loads
(runs 192ms, workflows 232ms, chats 326ms), live cron run events all arrive normally.

3. Confirm it is not a path problem:

```js
document.querySelector('link[rel="manifest"]')
// getAttribute('href') → "/t/28b0ccd5-.../manifest.webmanifest"   ← correctly re-rooted
// .crossOrigin         → null                                     ← the actual defect
```

## Root Cause

| # | Where | What |
|---|-------|------|
| 1 | `app/src/app.html:8` | `<link rel="manifest" href="/manifest.webmanifest" />` — **no `crossorigin` attribute** |
| 2 | `neboloop internal/api/tunnel.go:291` | hub rewrites the href to `/t/<botID>/manifest.webmanifest` — correct |
| 3 | browser | per the Web App Manifest spec, a manifest is fetched with credentials mode **`"omit"`** unless the link carries `crossorigin="use-credentials"` |
| 4 | browser | no `neboloop_tunnel` cookie and no `Authorization` header are sent |
| 5 | `neboloop internal/api/tunnel.go:108-135` | `tunnelProxy` finds neither Bearer nor cookie → `ownerID == uuid.Nil` → **401** |

The tunnel auth boundary is behaving exactly as designed. The client simply never presents a
credential for this one request.

Note the manifest body itself is already tunnel-correct — `start_url: "./"`, `scope: "./"`, and
relative icon paths resolve against the manifest's own URL, so under `/t/<botID>/` they land on
`/t/<botID>/` and `/t/<botID>/icons/icon-192.png`. Nothing else needs changing.

## Fix

One attribute, in `nebo`:

```html
<!-- app/src/app.html:8 -->
<link rel="manifest" href="/manifest.webmanifest" crossorigin="use-credentials" />
```

Safe in both deployments: the manifest is same-origin as the document in the tunneled case
(`neboai.com`) and in the direct case (`127.0.0.1:27895`). A same-origin URL fetched in CORS mode
skips the CORS check, so no response headers are required; the only behavioral change is that
cookies now ride along — which is precisely what the hub needs.

### Do NOT fix it hub-side

Allowlisting `manifest.webmanifest` past `tunnelProxy` auth would punch an unauthenticated hole in
the boundary that `docs/sme/TUNNEL.md` §3 establishes, for a cosmetic asset. The client-side fix is
smaller and keeps the boundary intact.

## Verification

After the change, through the tunnel:
1. `manifest.webmanifest` returns **200**, console is clean.
2. `Application → Manifest` in devtools shows name/icons/scope resolved under `/t/<botID>/`.
3. Add-to-Home-Screen on iOS Safari and Android Chrome installs a per-bot icon that launches into
   that bot's UI.

Test on a real phone, not just desktop devtools — the sliding tunnel session
(`tunnel.go:119-132`, re-minted per cookie-authed request, added specifically for "phone PWA") is
part of the path being exercised.

## Related

- `docs/sme/TUNNEL.md` §3 — tunnel auth boundary (Bearer or `neboloop_tunnel` cookie)
- `neboloop internal/api/tunnel.go:276-296` — the `ModifyResponse` HTML re-rooting block whose stated
  goal this bug defeats
