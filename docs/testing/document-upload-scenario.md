# Test Scenario: Document & Screenshot Upload (Comm Reply End-to-End)

**Status:** active · **First captured:** 2026-06-02 · **Spans:** `nebo` (agent + server) → NeboLoop comms → `neboloop` (frontend render)

This scenario tracks the full path an attachment travels when an agent produces a
file (typically a screenshot) inside an `Origin::Comm` run and replies to a user on
NeboAI.com / a loop. It is a cross-repo test: a single user-visible failure ("the
image is broken / the model says it failed") can originate in any of four places. All
four were live bugs on 2026-06-02; this doc is the regression guard.

> Companion: machine-readable assertions live in
> `docs/testing/strap-manifest.yaml` → `comm_session_behaviors:`. This doc is the
> human-readable repro + the cross-repo (neboloop) half the manifest doesn't cover.

---

## The pipeline (what "working" looks like)

```
user asks for a screenshot (NeboAI.com / loop chat)
  → agent run, Origin::Comm
     → os(resource:"capture", action:"capture")  OR  web(action:"screenshot")
        → tool result carries image_url
        → SIDECAR: vision-incapable main provider → image_url consumed, replaced
          with "[Page Visual] …" text analysis  (Anthropic/Gemini skip this)
        → save_data_uri_to_file → JPEG on disk
     → comm_manager.upload_file → POST NeboLoop /api/v1/files/upload
        → wire::Attachment { fileId, filename, mimeType, size, url, width, height }
     → SEND_MESSAGE with content.attachments[] (attachment_count >= 1)
  → NeboLoop router bindAttachments(): comms_attachment pending → attached
  → NeboLoop frontend: WS delivery → chat.ts normalize → AttachmentDisplay
     → image fetched WITH AUTH → blob URL → <img> renders
```

**End-state assertion:** the user sees the rendered image inline, and the model's
text truthfully says it captured/attached it — with **no** broken-image markdown and
**no** "I couldn't capture a screenshot."

---

## The four failure modes (and where each lives)

### Bug 1 — Hallucinated failure from context pollution *(nebo/agent)*
The model disbelieves its own successful tool results because the session already
accumulated earlier failures (sidecar errors, an `agent(action:"prompt")` hang, a
`loop(action:"share")` file-not-found). It tells the user "I'm unable to capture a
screenshot" while the logs show success.

- **Evidence (2026-06-02):** `16:10:28 save_data_uri_to_file uri_len=171183` ✓ ·
  `16:10:38 uri_len=167199` ✓ · `16:10:58 sending comm reply attachment_count=2` ✓ —
  model still reported failure.
- **This is the harness.md "Context Pollution Model" made concrete** — a Surface-2
  (response-interpretation) failure: success read as failure.
- **Fixes:** Bug #5 structural — neutral tool summaries + error decay (prevents
  accumulation in *future* sessions); `/new` clears an already-poisoned session
  (cannot be repaired in-session).
- **Assert:** with prior failures + a success in context, the model reports success.

### Bug 2 — Model can't tell the attachment is being delivered *(nebo/agent)*
The sidecar nulls `image_url` and swaps in `[Page Visual]` text. The model never sees
that the comm pipeline will attach the image, so even a willing model can't truthfully
say "screenshot attached."

- **Fix:** `crates/agent/src/runner.rs` — record `had_image` indices *before* the
  sidecar block; for `origin == Origin::Comm`, append to each such result:
  `"✓ Screenshot captured and will be delivered as an attachment in your reply to the user."`
- **Assert:** comm-origin runs whose tool result carried an `image_url` contain that
  exact note in the result content fed back to the LLM.

### Bug 3 — Broken markdown image in the reply text *(nebo/server)*
The model writes `![Screenshot](/local/abs/path.jpeg)` (or `file://…`) into the reply
text. The web frontend can't reach a local path → broken image, even though the real
image arrives as an attachment.

- **Evidence:** 2026-06-02 10.20 — broken markdown image rendered next to a working
  `attachment_count=3`.
- **Fix:** `crates/server/src/chat_dispatch.rs` — `strip_local_image_markdown()` runs
  before the comm reply is sent, when `comm_file_artifacts` is non-empty. It removes
  `![alt](url)` where `url` starts with `/` or `file://` (and the surrounding
  newline). Candidate prompt-side reinforcement: tell the model not to embed local
  markdown images on comm channels.
- **Assert (unit):** `strip_local_image_markdown` removes local/`file://` image refs,
  leaves `http(s)://` refs and normal text intact.

### Bug 4 — `os(action: "capture")` invalid action *(nebo/tools)*
The model confuses the `capture` *resource* with a top-level action.

- **Fix:** `crates/tools/src/desktop_tool.rs` — alias `"capture" => screenshot` in
  `handle_capture`; plus a `tool_corrections` entry. Correct calls: desktop =
  `os(resource:"capture", action:"capture")`; browser = `web(action:"screenshot")`.
- **Assert:** both spellings resolve to a screenshot; the correction fires on
  `os(action:"capture")`.

### Bug 5 — Attachment lost on the NeboLoop side *(neboloop/frontend — TWO distinct bugs)*

**5a. Dropped during stream finalization.** When the final non-streaming message
replaces the accumulated stream, the finalize branch copied `text`/`html` but **not**
`attachments`, so `content.attachments[]` was discarded for any streamed reply.
- **Fix:** `app/src/lib/stores/chat.ts` — both finalize branches (agent-space
  `:430`, channel `:372`) now carry
  `attachments: normalizeAttachments(content.attachments) ?? last.attachments`.

**5b. The `<img src>` auth gap (was still broken after 5a).** `AttachmentDisplay`
rendered `<img src="/api/v1/files/{uuid}">` directly, but that route is **bearer-only**
(`getUploaderID==nil → 401`) and **no auth cookie exists** (token is `localStorage`
`neboai_token`) and the service worker has **no `fetch` handler**. Browsers never send
`Authorization` on a subresource request → **401 → broken image**. This is why the
image stayed broken even after Bug 3 was stripped and Bug 5a was fixed.
- **Fix:** `app/src/lib/components/chat/AttachmentDisplay.svelte` +
  `chat.ts:getAttachmentBlobUrl` — fetch image/video bytes **with the bearer token**,
  render from an object URL (the same pattern `DocumentViewer` already used for PDFs).
  The generic download link was converted from `<a href>` to an authed
  `downloadAttachmentFile()` for the identical reason. (Presigned-URL alternative
  rejected: presigned URLs expire ~1h but chat history is permanent, so stored
  presigned URLs would rot.)
- **SME reference:** `neboloop/docs/sme/LOOPS_FILES_SME.md`.

---

## Manual repro (golden path)

1. On NeboAI.com, run `/new` (guarantees a clean, unpoisoned session).
2. Ask: "take a screenshot of <something on screen / a web page>."
3. Watch nebo logs: expect `save_data_uri_to_file uri_len=…` then
   `sending comm reply attachment_count>=1`, and no upload error.
4. In the chat: the image renders inline; the model's text says it captured/attached
   the screenshot; there is **no** broken-image icon and **no** `![Screenshot](…)`
   text artifact.
5. DevTools → Network: the image request resolves **200** (it should be a `blob:` URL,
   not a raw `GET /api/v1/files/<uuid>` — a raw one would be `401`).

## Negative / regression checks

| # | Inject | Expect |
|---|--------|--------|
| 1 | Poisoned session (prior failures) + a successful screenshot result | Model reports success, not failure (Bug 1) |
| 2 | Comm-origin screenshot result, post-sidecar | Result content contains the delivery note (Bug 2) |
| 3 | Reply text with `![x](/abs/path.png)` and `![y](https://h/z.png)` | Local stripped, http(s) kept (Bug 3) |
| 4 | `os(action:"capture")` | Resolves to screenshot / correction fires (Bug 4) |
| 5 | Streamed comm reply that finalizes, carrying attachments | `attachments` survive finalization (Bug 5a) |
| 6 | Rendered image request in browser | Authenticated fetch → blob → 200, not a bare 401 `<img>` (Bug 5b) |

## Gotchas for whoever automates this

- **Vision-capable vs not changes the path.** Anthropic/Gemini main providers pass the
  image through and skip the sidecar, so the `[Page Visual]` swap and the Bug 2 signal
  only matter for vision-incapable mains. Test both.
- **`attachment_count` in logs ≠ rendered.** It only proves nebo *sent* the
  attachment. Bug 5a/5b mean it can still not render — assert the rendered DOM, not the
  send count.
- **Test at a real viewport in a real browser.** A devtools `fetch()` carries auth and
  will look fine while the actual `<img>` tag silently 401s (Bug 5b).
- **`pending` attachments self-destruct in ~1h** (NeboLoop orphan GC) — don't reuse a
  stale `fileId` across a long-running test.
