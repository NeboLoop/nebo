# ADR-002: Cancel Active Task & Queue Visibility

**Status:** Proposed  
**Date:** 2026-02-07  
**Author:** Alma Tuck

## Context

When the agent is processing a request on the main lane, there is no way to cancel it. The user must wait for the full response to complete — even if they realize mid-stream they asked the wrong question, want to rephrase, or simply want to interrupt.

Additionally, when the user sends follow-up messages while the agent is busy, those messages are silently queued (both frontend-side in the Svelte `messageQueue` array and backend-side via lane queueing). The user has no visibility into queue state and no ability to remove queued messages.

### Current Gaps

1. **No cancel for active tasks.** `LaneManager.ClearLane()` cancels *queued* entries but not the *actively running* task. Each `laneEntry` has a `ctx + cancel` pair, but once a task starts executing in `pump()`, the entry leaves the `Queue` slice and becomes an anonymous goroutine with no external handle.

2. **No WebSocket cancel message type.** The realtime system handles `ping`, `chat`, `rewrite`, `approval_response`, `request_introduction`, and `check_stream` — but has no `cancel` type.

3. **No frontend stop button.** The chat UI shows a loading spinner during streaming but offers no way to abort. The send button is effectively disabled (messages get queued instead).

4. **No queue visibility.** Queued messages are invisible to the user. No count, no list, no ability to reorder or remove.

## Decision

### Phase 1: Lane System — Track Active Tasks

Modify `internal/agenthub/lane.go`:

- Add an `active []*laneEntry` field to `lane` (alongside the existing `queue`)
- In `pump()`, move entries from `queue` → `active` before executing, remove from `active` on completion
- Add `CancelActive(laneName string) int` — iterates `active` entries for the named lane, calls each entry's `cancel()`, returns count cancelled
- Add `CancelAll(laneName string) int` — cancels both active and queued entries
- Add `ActiveCount(laneName string) int` and `QueueCount(laneName string) int` for observability

The runner's `runLoop` already uses the lane entry's context. Cancelling it will:
1. Abort the in-flight AI provider stream (HTTP context cancellation)
2. Cause `runLoop` to return with a context cancellation error
3. Allow the lane to proceed to the next queued task

### Phase 2: Wire Cancel Through the System

**Backend (`internal/agenthub/agent.go`):**
- Handle a new `"cancel"` method in `handleAgentMessageWithState`
- Call `state.lanes.CancelActive(LaneMain)` to cancel the running task
- Send a `chat_cancelled` event back to the frontend so it can clean up UI state

**Realtime (`internal/realtime/`):**
- Add `SetCancelHandler()` + `handleCancel()` in `client.go`
- Register `"cancel"` in the message type switch
- The handler forwards the cancel request to the agent via the hub

**Message format (frontend → backend):**
```json
{
  "type": "cancel",
  "data": {
    "session_id": "current-session-id"
  }
}
```

**Response event (backend → frontend):**
```json
{
  "type": "chat_cancelled",
  "data": {
    "session_id": "current-session-id",
    "cancelled_count": 1
  }
}
```

### Phase 3: Frontend — Stop Button & Queue Visibility

**Stop button:**
- When `isLoading` is true, replace the send button with a stop button (⏹ icon)
- On click, send `{ type: "cancel" }` over WebSocket
- On receiving `chat_cancelled`, set `isLoading = false` and clear streaming state
- Append a system-style message: "Response cancelled"

**Queue indicator:**
- When `messageQueue.length > 0`, show a badge/count near the input area
- Optionally show queued message previews in a small dropdown
- Allow removing individual queued messages (click × to dequeue)

## File Changes

| File | Change |
|------|--------|
| `internal/agenthub/lane.go` | Add `active` tracking, `CancelActive()`, `CancelAll()`, counts |
| `internal/agenthub/agent.go` | Handle `"cancel"` method, emit `chat_cancelled` |
| `internal/realtime/client.go` | Register `"cancel"` message type |
| `internal/realtime/chat.go` | Add `SetCancelHandler()` + handler impl |
| `app/src/routes/(app)/agent/+page.svelte` | Stop button, queue badge, cancel handling |
| `app/src/lib/components/ChatInput.svelte` | Stop button variant (if input is a component) |
| `app/static/app.css` | Styles for stop button and queue badge |

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Cancelling mid-tool-execution could leave side effects (e.g., half-written file) | The runner already handles context cancellation gracefully — tools check `ctx.Done()`. File writes are atomic (write-then-rename). Shell commands get SIGTERM. |
| Race condition: cancel arrives just as task completes naturally | Use sync.Mutex on `active` slice. If cancel finds nothing active, it's a no-op. Frontend handles both `chat_complete` and `chat_cancelled` idempotently. |
| Cancelled AI response leaves partial message in history | Append the partial response as-is with a `[cancelled]` suffix. The user can see what was generated before cancellation. |
| Queue manipulation (removing items) while new messages arrive | Frontend queue is a Svelte `$state` array — reactivity handles this. Backend queue is mutex-protected. |

## Alternatives Considered

1. **Browser-side only cancel (abort the SSE/WS read).** Doesn't actually stop backend work — the agent keeps streaming to nobody. Wastes API credits and blocks the lane.

2. **Kill the entire agent and reconnect.** Sledgehammer approach. Loses all lane state, pending sub-agents, and in-flight work on other lanes.

3. **Timeout-based auto-cancel.** Doesn't solve the interactive "wrong question" case. Could be added later as a complementary feature.

## Success Criteria

- [ ] User can click stop mid-stream and the response halts within 2 seconds
- [ ] Queued messages are visible and individually removable
- [ ] Cancelling does not corrupt conversation history
- [ ] Lane system correctly tracks active vs queued counts
- [ ] No regression in normal chat flow (send → stream → complete)
