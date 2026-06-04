# Bug: @mentioned agent ("pam") never responds — A2A delivery dies silently

**Date:** June 4, 2026
**Severity:** High (core A2A / multi-agent feature appears broken to the user)
**Component:** `crates/comm/src/neboai.rs`, `crates/server/src/lib.rs` (comm routing),
`crates/server/src/handlers/ws.rs` (local @mention fork), `crates/server/src/chat_dispatch.rs`
**Status:** Logged, not yet fixed (handed to a dedicated A2A agent)

## Symptom

User @mentions another agent ("pam") from Nebo and never gets a response. No error is
surfaced anywhere in the UI.

## Two distinct @mention pathways — first determine which one "pam" is

The @mention syntax (`<@id>`, parsed in `ws.rs:parse_mention_tokens` ~1758-1786 and regex
`MENTION_TOKEN_RE` at `lib.rs:59-63`) fans out to **two completely different delivery paths**.
Diagnosis must start by establishing which "pam" is:

1. **LOCAL agent** (installed on this Nebo): handled by `fork_mention_chat()` in
   `ws.rs:1792-1881`. Resolves the id via `state.store.get_agent()`, auto-activates
   (`ws.rs:1802-1839`), runs a local chat, then `inject_delegate_response()` (`ws.rs:1883-1957`)
   pushes the reply back into the primary session.
2. **REMOTE bot agent over NeboLoop A2A** (a different bot owns "pam"): handled by the comm
   send path — `chat_dispatch::send_to_channel()` → `NeboAIPlugin::send()` (`neboai.rs:584-671`),
   addressed by `agent_space_by_slug` or `dm_by_peer`.

**Diagnostic:** grep the server log for these strings:
- `mentioned agent not found, skipping` → LOCAL path, failure mode #1 below.
- `no conversation for pam` → REMOTE path, failure mode #2 below (most likely).

## Root causes (suspected, ranked)

### #1 — REMOTE: agent_space JOIN never completed → send() drops silently (MOST LIKELY)

`NeboAIPlugin::send()` resolves the destination conversation at `neboai.rs:591-606`:

```rust
} else if !msg.to.is_empty() {
    let maps = self.conv_maps.read().await;
    if let Some(conv) = maps.agent_space_by_slug.get(&msg.to) {
        conv.clone()
    } else {
        maps.dm_by_peer.get(&msg.to).cloned()
            .ok_or_else(|| CommError::Other(format!("no conversation for {}", msg.to)))?
    }
}
```

If `agent_space_by_slug` has no entry for "pam", `send()` returns `Err`. That error is only
logged at `warn` in `chat_dispatch.rs` (~780: `warn!(error = %e, "failed to send comm reply")`)
and **never surfaced to the user** — the message is silently dropped.

Why the map would be empty: on `connect()` the bot only joins
`["dm", "installs", "chat", "account", "voice"]` (`neboai.rs:520-548`). **Agent-space
conversations are NOT joined on startup** — they're learned on-demand from
`JOIN_CONVERSATION` responses (`neboai.rs:1148-1194`). If that response was lost/never sent
(gateway hiccup, registration race), the slug→conv mapping never exists and every future
mention to "pam" fails.

### #2 — REMOTE: hot-reload race resets ConvMaps

`connect()` resets `*maps = ConvMaps::default()` (`neboai.rs:459-461`). On hot-reload (our
`make dev` SIGKILL pattern), if the OLD read loop processes a `JOIN_CONVERSATION` *after* the
NEW connection reset the maps, the mapping lands on the dead instance — the live one stays
empty → same "no conversation for pam" silent drop. (See also memory note: ghost connections
from hot-reload, no graceful close on SIGKILL.)

### #3 — REMOTE: recipient bot offline → silent drop, no ACK

There is no delivery ACK. `send()` returns `Ok(())` once the frame is queued to the WS write
loop (`neboai.rs:668-670`); it does not verify the recipient bot is connected. If the bot
hosting "pam" is offline, the gateway drops the frame and nothing is surfaced.

### #4 — LOCAL: agent not found → early return

If "pam" is meant to be local but isn't in `store.get_agent()`, `fork_mention_chat` logs
`warn!("mentioned agent not found, skipping")` and returns at `ws.rs:~1835` — no message, no
user-facing error.

## Common thread / real fix direction

Every failure mode is **silent** — the user gets no signal that delivery failed. Proper fixes:

1. **Surface send failures.** Propagate `CommError` from `send()` through `chat_dispatch` so
   the user sees "couldn't reach pam (offline / not joined)" instead of nothing. This is the
   highest-value fix regardless of which root cause is active.
2. **Auto-rejoin agent_spaces on startup/reconnect.** Fetch the agent list from the NeboLoop
   API and explicitly join all registered agent_space conversations, instead of relying on
   on-demand `JOIN_CONVERSATION` responses. Closes #1 and #2.
3. **Fix the ConvMaps reconnect race.** Use a generation counter (or atomic swap) so a late
   JOIN response from a stale read loop is discarded rather than written to a dead map.
4. (Longer-term) **Delivery ACK** so the sender knows whether the recipient's agent loop
   actually received the message.

## Reproduce

1. From Nebo, @mention an agent owned by another bot ("pam").
2. Observe: no reply, no error in UI.
3. Check server log for `no conversation for pam` (remote) vs `mentioned agent not found, skipping` (local).

## Notes

- Investigated 2026-06-04 during the prompt-optimization (ExecutionMode) work; unrelated to
  those changes. Cross-references memory notes on NeboLoop connection bugs
  (missing-agent_space-join, hot-reload ghost connections, no-mutex on activate_neboloop).
- Key files for the fixing agent: `crates/comm/src/neboai.rs` (send 584-671, connect
  459-548, JOIN handling 1148-1194), `crates/server/src/lib.rs` (`handle_comm_message`
  routing ~2377-3241), `crates/server/src/chat_dispatch.rs` (`send_to_channel` ~1262-1289,
  warn-only error ~780), `crates/server/src/handlers/ws.rs` (`fork_mention_chat` 1792-1957).
