# Bug Report: Loop Channel Messages Not Appearing

**Status:** ✅ FIXED & VERIFIED — June 11, 2026 (~11:00 PM MDT)

**Date:** June 11, 2026  
**Time:** 10:47 PM MDT  
**Reporter:** Alma's Assistant  

## Root Cause
The loop tool's channel `send` (`crates/tools/src/loop_tool.rs`) builds a
`CommMessage` with `conversation_id = channel_id` and `topic/stream = channel_id`
(the raw channel UUID). But the gateway routes channel messages by the channel's
**distinct** `conversation_id` (resolved at join time into `channel_convs`) on the
fixed `stream = "channel"`. `comm.send()` never resolved `channel_id → conversation_id`,
so the WS `SendPayload` targeted a conversation that doesn't exist and the gateway
silently dropped it. Reads worked because `list_channel_messages` is a REST call
keyed by `channel_id`. The send is fire-and-forget (`Ok` = frame queued, no server
ack), so it reported a false success.

Proven by the logs — a working agent channel reply vs. the broken loop send:
```
WORKING (Jim reply): stream=channel   conv_id=fc558e1f-...  topic=channel
BROKEN  (loop tool): stream=90fca427... conv_id=90fca427... msg_type=LoopChannel
```

## Fix
`crates/comm/src/neboai.rs` — `send()` now detects `LoopChannel` messages and
resolves `channel_id → conversation_id` via `channel_convs` (joining the channel
first if not yet mapped, with a bounded wait via the new `resolve_channel_conv`),
and forces `stream = "channel"`. If it can't resolve (bot not a member), it returns
an **error** instead of a false success.

## Verification (live)
Sent "Loop channel fix verification 225908" to #general via the loop tool:
- Log: `conv_id=fc558e1f-... stream=channel msg_type=LoopChannel` (resolved ✓)
- Channel history (`messages` read): the message now appears (`created_at 04:59:08`)

Remaining nicety (not blocking): `from`/`senderName` is empty, so a loop-sent
message attributes to the primary bot rather than the specific agent. Fixing that
means threading the agent's display name into the send as `metadata.senderName`.


## Issue Description
Messages sent via the `loop` tool to channels are returning success responses but not appearing in the channel view.

## Evidence

### Tool Response
```
Message sent to channel 90fca427-3990-40b5-b5b2-a162287e0680
```

### Test Messages Attempted
1. "Test message" - Sent successfully per tool response
2. "Test message from Alma's Assistant - June 11, 2026" - Sent successfully per tool response

### Channel Verification
Retrieved last 5 messages from #general channel (channel_id: 90fca427-3990-40b5-b5b2-a162287e0680):
- Last message timestamp: 2026-06-12T01:32:09.171721Z
- Neither test message appears in the channel history

## Steps Reproduced
1. Called `loop(resource: "channel", action: "send", channel_id: "...", text: "...")`
2. Received success response: "Message sent to channel [id]"
3. Verified channel messages via `loop(resource: "channel", action: "messages", ...)`
4. Confirmed message not present in channel history

## Impact
Users cannot verify that messages were actually delivered to channels, breaking trust in the communication system.

## Suggested Investigation
- Check if messages are being queued asynchronously
- Verify if there's a delay before messages appear
- Investigate potential caching issues on the client side
- Review server-side message delivery logs

---
Task ID: 0b586127-2b0e-4f7d-9a58-ec0833e6f1dd
