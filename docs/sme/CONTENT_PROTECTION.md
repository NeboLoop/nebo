# Content Protection — Encrypted .napp at Rest

## Problem

Publishers create valuable skills (SKILL.md), agents (AGENT.md, agent.json) and list them on the marketplace for real money ($197 one-time, $9.97/mo). After install, everything lives as plain text on disk:

```
~/.nebo/nebo/skills/@acme/sales-qual/1.0.0/
  SKILL.md          <- the IP (prompt engineering, templates, instructions)
  manifest.json     <- metadata
~/.nebo/nebo/agents/@acme/closer/1.0.0/
  AGENT.md          <- persona, system prompt
  agent.json        <- workflow config
```

Anyone can `cat` these files and redistribute them. The .napp envelope has signature verification, but once extracted, it's over.

## Goal

Raise the bar against casual copying. Not perfect DRM — at some point the content is sent to an LLM and exists in memory. But make `cp ~/.nebo/ /tmp/stolen/` and forum sharing impractical.

## Architecture: Bot-Bound Encrypted .napp, Never Extracted

1. **On install**, NeboAI wraps the .napp payload with a second encryption layer — AES-256-GCM keyed to the buyer's bot_id or owner_id (depending on license scope)
2. **On disk**, only the encrypted .napp exists — no sibling extracted directory for paid content
3. **At runtime**, Nebo decrypts in memory using a key derived from bot_id/owner_id + a per-artifact secret (fetched once from NeboAI, stored in keyring)
4. **Copying the .napp** to another machine = useless ciphertext (wrong bot_id or wrong account)

### The .napp Envelope Becomes Two Layers

```
[NAPP magic + ED25519 sig + SHA256]     <- integrity + origin (existing)
  [AES-256-GCM(license_key)]            <- confidentiality (new)
    [tar.gz payload]                     <- SKILL.md, AGENT.md, etc.
```

## Two License Scopes

| Scope | Key bound to | Use case | Transfer? |
|-------|-------------|----------|-----------|
| **Per-user** | `owner_id` | Personal use across machines — "I bought it, all my bots get it" | Automatic — any bot you own works |
| **Per-bot** | `bot_id` | Team/enterprise — "This license is assigned to the Sales bot" | Manual — reassign via dashboard |

Key derivation changes based on scope:

```
per-user:  HKDF(master_secret, owner_id + artifact_id)
per-bot:   HKDF(master_secret, bot_id + artifact_id)
```

## Bot Registry (NeboAI Side)

Bots are currently ephemeral — they show up via CONNECT frame and API headers, but NeboAI doesn't formally own a registry. That needs to change.

```sql
bots
  id              UUID        -- the bot_id (generated locally, registered on first connect)
  owner_id        UUID        -- NeboAI user account
  name            TEXT        -- user-assigned ("Work Mac", "Home Desktop", "Sales Bot")
  platform        TEXT        -- "darwin-arm64", "linux-amd64"
  app_version     TEXT        -- "0.9.0"
  first_seen      TIMESTAMP
  last_seen       TIMESTAMP
  status          TEXT        -- active | inactive | transferred

licenses
  id              UUID
  artifact_id     UUID        -- the skill/agent/plugin
  artifact_type   TEXT        -- "skill", "agent", "plugin", "bundle"
  owner_id        UUID        -- the purchaser
  scope           TEXT        -- "user" or "bot"
  bot_id          UUID NULL   -- only for per-bot licenses
  purchased_at    TIMESTAMP
  expires_at      TIMESTAMP NULL  -- null = perpetual (one-time purchase)
  subscription_id UUID NULL       -- links to billing for recurring
  status          TEXT        -- active | expired | revoked | transferred
```

## Key Distribution Flow

```
Bot starts up -> authenticates to NeboAI (JWT with owner_id)
  -> requests decryption keys for installed artifacts
  -> NeboAI checks:
      per-user scope: does owner have active license? -> return key
      per-bot scope:  does THIS bot_id match the licensed bot? -> return key
  -> keys cached in local keyring with TTL
      one-time purchase: 30-day TTL (refresh is just a validity check)
      subscription: TTL matches billing cycle (monthly refresh)
  -> if offline: cached key works until TTL expires
```

## Transfer / Reassignment

**Per-user licenses** — no transfer needed. Buy a new machine, install Nebo, log in, your bots auto-register, keys resolve.

**Per-bot licenses** — explicit reassignment:

1. User opens NeboAI dashboard -> Licenses
2. Selects license -> picks destination bot from their bot registry
3. NeboAI revokes old key, issues new key bound to new bot_id
4. Old bot: next key refresh fails -> content locks
5. New bot: requests key -> gets fresh key -> content unlocks

**Hardware replacement** (new machine, same use):

1. Install Nebo -> new bot_id generated
2. Log in -> new bot registers under same owner
3. Per-user licenses: work immediately
4. Per-bot licenses: user reassigns from old bot to new bot in dashboard
5. Old bot can be deactivated

## What This Enables Beyond DRM

The bot registry isn't just about content protection — it unlocks:

- **Multi-device sync** — know which bots a user owns, sync settings/preferences
- **Team management** — org admin assigns licenses to specific team bots
- **Usage analytics** — per-bot usage tracking for enterprise billing
- **Remote wipe** — revoke all keys for a stolen/compromised device
- **Fleet management** — "push this agent to all my bots" vs "only the sales bot"

## The Publisher Experience

When listing on the marketplace:

```
Pricing:
  [ ] One-time purchase     $___
  [ ] Monthly subscription  $___/mo

License scope:
  ( ) Per-user (buyer uses on all their bots)
  ( ) Per-bot (buyer assigns to one bot, can reassign)
  ( ) Buyer chooses at checkout
```

## What Gets Protected vs What Doesn't

### Protected (encrypted at rest, decrypted only in memory)

- SKILL.md bodies (prompt engineering, templates, instructions)
- AGENT.md personas (system prompts, persona definitions)
- agent.json (workflow configurations)

### Not Protected (no encryption needed)

- **Free content** — no encryption, extracts normally (bundled skills, open-source agents)
- **Plugin binaries** — compiled code, not readable IP
- **Metadata** (`manifest.json`, `plugin.json`) — needed for tool registration and discovery
- **Plugin skills embedded in .napp** — follow same rules as standalone skills (free = extract, paid = sealed)

## What Already Exists

| Component | Status | Location |
|-----------|--------|----------|
| `read_napp_entry()` / `read_napp_entry_string()` | Done | `crates/napp/src/reader.rs` — reads from archive in memory |
| AES-256-GCM encryption | Done | `crates/mcp/src/crypto.rs` |
| Keyring integration | Done | `crates/auth/` |
| bot_id persistence | Done | `crates/config/src/defaults.rs` |
| .napp envelope (ED25519 + SHA256) | Done | `crates/napp/src/napp.rs` |
| Skill loader reads SKILL.md into memory | Done | `crates/tools/src/skills/loader.rs` — lazy template loading |
| NEBOAI_PUBLIC_KEY embedded | Done | `crates/napp/src/signing.rs` |

## What Would Need to Change

### NeboAI Side

- Bot registry API (register, list, deactivate, transfer)
- License management (create on purchase, check on key request, revoke on lapse)
- Key distribution endpoint (authenticated, returns per-artifact decryption keys)
- Install endpoint encrypts .napp with buyer's license key before delivery
- Subscription webhook revokes/refreshes keys on billing events

### Nebo Client Side

1. **Sealed .napp reader** — `unwrap_sealed_napp(data, license_key)` in `napp.rs`: AES-256-GCM decrypt -> then `unwrap_napp()` for signature verify
2. **Bot registration** — on first connect (or account link), register bot_id with NeboAI
3. **Key cache** — store decryption keys in keyring with TTL, refresh on startup
4. **Skill/agent loaders** — read from sealed .napp directly (no extract-alongside for paid content)
5. **Phase 3 extraction** — skip sealed .napp files (they stay sealed)
6. **Free vs paid detection** — check manifest or .napp header for encryption flag

## Threat Model

### Protected against

- Casual file copying (`cp`, `tar`, file manager)
- Forum/torrent redistribution of .napp files
- Subscription circumvention (key expires with billing)
- Stolen device (remote key revocation)

### Not protected against (acceptable)

- Process memory dumping by a skilled attacker
- Modified Nebo binary that dumps decrypted content
- Screenshots of decrypted content displayed in UI

The goal is marketplace-grade protection, not military-grade DRM. If someone reverse-engineers the binary to extract prompts, they've spent more effort than the $197 purchase price.
