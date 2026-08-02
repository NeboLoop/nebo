# Comms Wire Contract Has No Shared Source of Truth

**Repos:** `nebo` + `neboloop` (cross-repo)
**Found:** 2026-07-28
**Priority:** Medium — no live defect, but the contract is enforced only by two teams remembering.

## Issue

Both repos carry a `proto/comms/v1/comms.proto`. Neither is generated from the other, neither is
compiled into the build, and they describe **different protocols**. The file that both sides *appear*
to share is stale documentation that disagrees with itself and with the running code.

The real wire contract lives in two hand-written, hand-mirrored places:

| Side | File | Form |
|------|------|------|
| Nebo (Rust) | `crates/comm/src/wire.rs` | serde structs, camelCase JSON payloads |
| NeboLoop (Go) | `internal/comms/` (wire types) | Go structs, camelCase JSON payloads |

The transport is a 47-byte binary header + **JSON** payload (`crates/comm/src/frame.rs`,
`docs/sme/NEBOLOOP_CONNECTION.md` §2). Protobuf is not on the wire at all.

## Evidence

```
nebo/proto/comms/v1/comms.proto        2754 bytes   May 28 12:15
neboloop/proto/comms/v1/comms.proto    2475 bytes   Feb 16 16:30
```

`diff -r` shows near-total divergence — not drift in one field, but different message names and
different semantics:

| Concept | nebo's proto | neboloop's proto |
|---------|--------------|------------------|
| Auth response | `AuthOKPayload {session_id, loop_id, roles}` + separate `AuthFailPayload` | `AuthResult {ok, reason, bot_id}` (one message) |
| Send | `SendPayload {content_type, body, correlation_id, stream}` | `SendMessagePayload {conversation_id, stream, content, correlation_id, metadata}` |
| Delivery | `DeliveryPayload {from, content_type, body, ...}` | `MessageDelivery {conversation_id, stream, content, sender_id, seq, msg_id, metadata}` |
| Join | `JoinPayload {map<string,uint64> last_acked_seqs}` | `JoinConversation {conversation_id, last_acked_seq}` |
| Ack | `AckPayload {last_seq}` | `Ack {conversation_id, acked_seq}` |
| `go_package` | `github.com/neboloop/nebo/internal/neboloop/sdk/pb` | `github.com/NeboLoop/neboloop/internal/comms/pb` |

nebo's version also carries typed bodies (`InstallEvent`, `ChannelMessage`, `TaskSubmission`,
`TaskResult`, `DirectMessage`) that neboloop's does not mention at all.

Neither matches `wire.rs`, which is what actually ships: e.g. the live `AuthResultPayload` is
`{ok, reason, botId, plan, token}` — the `plan` and `token` fields (JWT rotation, the mechanism the
whole reconnect path depends on) appear in **neither** proto.

## Why It Matters

1. **A reader will trust it.** The file is named, versioned, and sits in a `proto/` directory in both
   repos. Someone implementing a third client (mobile, SDK, a partner bot) will generate from it and
   produce something that cannot connect.
2. **No compile-time coupling.** Adding a field to `wire.rs` and forgetting the Go struct is a silent
   runtime `null`, not a build failure. Same in reverse.
3. **It masks the real contract.** The authoritative definition is currently `docs/sme/NEBOLOOP_CONNECTION.md`
   plus two source files — discoverable only if you already know to look.

## Expected

One of two end states, not the current third one:

- **(a) Generate both sides from one file.** Single `comms.proto` in one repo, vendored or submoduled
  into the other, `make gen` emits Rust and Go types. Contract drift becomes a build error. Note this
  implies moving the payload from JSON to protobuf on the wire, or generating serde/JSON types from
  the proto — decide which.
- **(b) Delete both protos.** Declare `wire.rs` the source of truth, document that Go mirrors it by
  hand, and add a drift test. Cheaper, honest, and consistent with how the code actually works today.

## Suggested Fix

Recommend **(b)** unless a third-party client is on the roadmap — the JSON wire is already shipping and
working, and (a) is a protocol migration disguised as a cleanup.

Concretely for (b):
1. `git rm nebo/proto/comms/ neboloop/proto/comms/`.
2. Add a header comment to `crates/comm/src/wire.rs` naming it the source of truth and pointing at the
   Go mirror.
3. Add a drift test that asserts the serialized field set of each payload struct matches a checked-in
   golden JSON fixture, in **both** repos, from the same fixture file. A field added on one side and
   not the other then fails CI.

If (a) is chosen instead, it is a scheduled protocol change, not a docs task — size it accordingly.

## Related

- `docs/sme/NEBOLOOP_CONNECTION.md` — current authoritative description of the wire
- `issues/cross-repo-tunnel-denylist-drift.md` — same root cause (two repos agreeing by convention,
  not by construction), higher stakes
