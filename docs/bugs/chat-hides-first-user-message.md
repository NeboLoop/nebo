# Bug: chat hides the original user message after a new thread is auto-titled

**Date:** June 2, 2026
**Severity:** Medium (UX / data-display)
**Component:** `app/src/lib/chat/controller.svelte.ts` (chat message store), `ChatPane.svelte`
**Status:** Logged, not yet fixed (deferred — focus is on prompt/tools)

**Description:**
In a brand-new thread, after the assistant returns its first response and the thread gets
auto-titled (e.g. "Sundance UT Temperature Forecast"), the chat pane renders ONLY the
assistant response — the user's original question bubble is gone from the view.

**Root cause (suspected, not yet confirmed):**
`controller.svelte.ts` resets the message list to `[]` in several paths (`:363`, `:462`,
`:502`) and reloads/prepends history (`prependMessages`, `:498`). When the new chat is
auto-titled, the thread is likely reloaded/replaced and the first user turn is dropped in
the reload — the same family as the streamed-attachment finalize drop documented in
`docs/testing/document-upload-scenario.md` (Bug 5a: finalize branch copied text/html but
not all fields). There are also 22 uncommitted lines in `controller.svelte.ts` from before
the current session that may be involved.

**Steps to Reproduce:**
1. Start a new thread (`/new`).
2. Send a question that triggers a web search / longer response.
3. When the response lands and the thread title is generated, observe the chat pane.

**Expected:** The thread shows the user's question followed by the assistant's answer.

**Actual:** Only the assistant answer is shown; the user's question is hidden.

**Proposed fix (for later):**
Audit the new-chat → first-response → auto-title → reload path in
`controller.svelte.ts`. Ensure the reload/prepend merges (does not replace-and-drop) the
in-flight user message, and that finalize carries the full message list. Confirm against
the uncommitted diff in that file.

**Notes:** Not caused by this session's changes — all of those were backend (Rust prompt /
registry / runner / tool_filter). This is a frontend chat-store issue.
