# Reverse Management Tunnel — SME Reference

> **Status:** Built and verified (2026-07). Nebo `468c5e03` + `7d035f58`, neboloop `bbb3c72` + `34764f0`.
> **Design doc:** `docs/plans/nebo-cloud-architecture.md` (local, gitignored) — "Plane B."
> **One-line model:** every bot (desktop *and* cloud) dials the hub outbound; the loop reverse-proxies
> the owner's browser into that connection. **The tunnel is the new localhost.**

---

## 1. What It Is

Nebo's backend trusts localhost: nearly all of its ~250 REST endpoints and its `/ws` socket are
unauthenticated by design. To manage a bot from the loop (neboai.com), we do NOT re-authenticate the
backend; we move the trust boundary. The bot opens **one outbound `wss` connection** to the hub and runs
**yamux** over it. Per browser request, the hub opens a new mux stream and raw-proxies HTTP/WS to the
bot's local `127.0.0.1:27895`. All endpoints — present and future — work unchanged, with zero
per-endpoint work.

This is separate from the comms bus (Plane A, `NEBOLOOP_CONNECTION.md`): comms is a durable 32KB-framed
message log; the tunnel is a raw byte pipe for the management/UI path.

## 2. Components

| Side | File | Role |
|------|------|------|
| Nebo (Rust) | `crates/comm/src/tunnel.rs` | Tunnel client: `run(hub_url, token, local_addr)` — dial, mux, gate, proxy |
| neboloop (Go) | `internal/tunnel/hub.go` | Hub: bot registry, yamux session per bot |
| neboloop (Go) | `internal/api/tunnel.go` | `tunnelConnect` (bot side), `tunnelProxy` (browser side), `createTunnelSession` |
| neboloop (Go) | `internal/api/router.go:115-124, 334` | Routes (root-level, NOT under `/api/v1`) |

**Routes:**
- `GET /tunnel/connect` — the bot dials this with its bot token; upgraded to WS, wrapped in yamux (hub is the yamux *client*, bot is the *server* — hub opens streams).
- `ANY /t/{botID}/*` — owner-authed reverse proxy into that bot's mux. Auth = Bearer **or** the `neboloop_tunnel` session cookie (HttpOnly, SameSite=Lax, `Path=/t/`), minted by `POST /api/v1/tunnel/session` for WS upgrades and other requests that can't carry a header.

**Wire framing:** one binary WS frame per write on both sides. Rust adapts via a custom `WsIo`
(AsyncRead/AsyncWrite over tungstenite); Go uses gobwas/ws + wsutil. rust-yamux 0.13 ↔ hashicorp/yamux
v0.1.2 interop is confirmed end-to-end (curl through hub → bot, incl. streaming and WS upgrade).

## 3. Security Model (layered, both sides)

The tunnel is the auth boundary. Defenses, in request order:

1. **Owner auth at the hub** — `tunnelProxy` requires the bot's owner (JWT or tunnel session cookie) before opening a stream. `ListBotsByOwner`-style ownership is checked; strangers never reach the mux.
2. **Hub-side denylist** — `isBlockedTunnelPath` (`internal/api/tunnel.go`) refuses `/ws/extension` and `/api/v1/update/` before proxying.
3. **Bot-side hub verification** — `verify_hub_url` (`tunnel.rs`) refuses any non-`wss://` hub URL except loopback (dev). TLS authenticates the hub; the bot presents its bot token.
4. **Bot-side denylist** — `is_blocked_path` mirrors the hub's list. The bot re-gates **every** request on a stream, not just the first: `proxy_stream` reads the full HTTP head, and `force_connection_close` rewrites keep-alive on non-upgrade requests so a hostile/compromised hub cannot pipeline a blocked request behind an allowed one on a reused stream. Blocked → clean `403` + `stream.shutdown()` (never a reset). WS upgrades are exempt from forced close (that's how `/ws` chat streams).
5. **Local WS endpoint auth** (defense in depth if anything ever reaches localhost):
   - `/ws/extension` — rejects any request with an `Origin` header and requires the per-install `X-Nebo-Extension-Secret` (`<data_dir>/.extension-secret`, 0600, constant-time compare, fail closed). See `crates/server/src/handlers/ws.rs`.
   - `/ws`, `/ws/app/{id}`, `/api/v1/agent/ws` — `origin_is_trusted` guard: an `Origin` header must be loopback/Tauri or the handshake gets 403. Native clients send no Origin and pass.
   - Extension tool results are bound to the connection they were routed to (`ExtensionBridge::deliver_result`), so even an authenticated second relay can't forge results.

**Blocked-path list (keep both sides in sync):** `/ws/extension`, `/api/v1/update/`. Bot side:
`tunnel.rs::is_blocked_path`; hub side: `tunnel.go::isBlockedTunnelPath`. If you add a
local-trust-only surface, add it to BOTH.

## 4. ⚠️ Phase-4 Gotcha: the hub MUST strip `Origin` (not yet done)

The Origin guard added to `/ws` (and friends) rejects any non-loopback/Tauri `Origin`. Today that is
correct — nothing legitimate reaches those endpoints from a remote origin. **But when Phase 4 lands
(the Nebo SPA served from the loop, proxying `/t/{botID}/ws` through the tunnel), the browser's WS
handshake will carry `Origin: https://…neboai.com`, and the bot will 403 it.**

**Required fix, hub-side, when Phase 4 starts:** `tunnelProxy` must strip (or rewrite to nothing) the
`Origin` header before writing the request into the mux. The hub has already authenticated the owner,
so dropping Origin is safe — and it must NOT be done for `/ws/extension` (that stays blocked at the
tunnel entirely). This note also lives as a code comment on `origin_is_trusted`
(`crates/server/src/handlers/ws.rs`) and in commit `51ab5ee3`.

Symptom if forgotten: remote UI loads, REST works, but the live socket never connects — bot logs
`ws: rejected upgrade from untrusted origin`.

## 5. Uniformity Rule

Desktop and cloud bots use the **identical** path: outbound tunnel + comms. A cloud pod is "a desktop
install that happens to run in k8s" — same image, no ingress/Service/oauth2-proxy/DNS per tenant. Only
the Tauri-embedded SPA takes the localhost fast path; the interface is the same.

## 6. Verification Recipe

1. Unit: `cargo test -p nebo-comm` (`blocks_local_trust_surfaces`, `requires_tls_hub_except_loopback`, `forces_connection_close`); Go: `go test ./internal/tunnel/ ./internal/api/`.
2. Live spike: run `nebo serve`, dial a local hub (`ws://127.0.0.1:<port>` is allowed for dev), then through the hub: an arbitrary `/api/v1/*` GET (expect same body as direct localhost), `/ws` upgrade + chat token stream, and `/ws/extension` (expect `403` **from the bot**, backend never reached).
