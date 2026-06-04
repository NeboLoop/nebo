# Work Pane — Agent-Produced Outputs (Design + Research)

Status: **research complete, not yet built** · Owner: TBD · Verified 2026-06-04

The "Work" pane (renamed from "Creations" in `c96c2bbf`) is the surface where a user sees
the things Nebo *produces* — reports, sheets, charts, images, designs. Today the pane is a
**stub**: `app/src/lib/components/chat/ChatPane.svelte` has `const artifacts: Artifact[] = []`
("populated by agent tool results in the future"), so it always reads "Nothing here yet."

This doc captures the research across the five subsystems this touches and the
CODE_AUDITOR-clean build plan (reuse the canonical pathway; no competing pathways; no
term collision).

---

## 1. Terminology — "artifact" is already overloaded (do NOT add a 3rd meaning)

| Meaning | Where | Storage |
|---|---|---|
| **Marketplace install + version/update tracking** (skills/plugins/agents) | `crates/server/src/handlers/artifact_updates.rs`, `/api/v1/artifacts/{check-updates,updates,apply-update,update-settings}` | DB table `artifact_update_prefs` (migration 0093); `db::models::ArtifactUpdatePref` |
| **Ephemeral produced-file URLs** (screenshots, generated files) | `chat_dispatch.rs` — `app_file_artifacts`, `to_app_artifact_url()`, the `chat_complete` `artifacts[]` WS field | none (on-disk under `<data_dir>/files/`, referenced per-message) |
| **Frontend chat artifact (proposed Work item)** | `ChatPane.svelte` `interface Artifact { id, messageId?, title, kind, preview }` | in-memory stub |

**Verdict:** the marketplace `artifacts` table is a *different concept*. **Do NOT create a new
`artifacts` DB table** for Work items — that's the CODE_AUDITOR rule-8 collision. **Reuse the
`chat_dispatch` produced-file "artifact" pathway** (meaning #2), which already exists and is
exactly "a tool produced a file → surface it to the app." If we ever need persistence beyond
on-disk + per-message, name the table `work_items` / `produced_files`, never `artifacts`.

---

## 2. The canonical produced-file pathway (REUSE this; don't reinvent)

```
tool sets ToolResult.image_url = <path|data:uri|url>     (crates/tools/src/registry.rs:63)
  → StreamEvent.image_url  (runner.rs:3007,3058 passthrough; ai/types.rs:85-89)
  → chat_dispatch on ToolResult event (chat_dispatch.rs:483-494):
        to_app_artifact_url() normalizes → /api/v1/files/<name>   (1159-1182)
        materializes: data: URI decoded, or local file COPIED into <data_dir>/files/
  → broadcast "chat_complete" { artifacts: [/api/v1/files/...] }  (845-851)
  → frontend WS listener (app/src/lib/websocket/listeners.ts:144-149)
  → controller.svelte.ts handleChatComplete: artifactsToAttachments(data.artifacts) (232-265)
  → ChatPane renders attachments (img/video/download)  (ChatPane.svelte:542-577)
```

**Already reusable:** `image_url` signal, `to_app_artifact_url()` normalize, data-URI/local-file
materialization into `<data_dir>/files/`, `chat_complete` artifacts array, `artifactsToAttachments`,
attachment rendering, and `GET /api/v1/files/*path` (serves `<data_dir>/files/`, files.rs:146-181).

**The gap:** the things users most want in Work — the deep-research **report.md** and `system`
**file writes** — return their path in `ToolResult.content` *text* only, NOT in `image_url`. So
they never enter the pathway. And the artifact payload is a bare URL with **no title/kind**.

---

## 3. A2UI "Workspace" vs "Work" — keep separate

- **A2UI / "Workspace" tab** (`crates/server/src/a2ui.rs`, `app/src/lib/.../a2ui/`, `a2ui.ts`,
  `workspaceOpen`): declarative, *interactive*, agent-driven UI (18 components: dashboards, forms,
  wizards). Ephemeral, agent-session-scoped, component-driven, action round-trips.
- **"Work" pane**: *static* produced files (markdown report, CSV, PNG, PDF). Persistent, user-owned,
  file-driven, view/download.

**Recommendation (confirmed by research):** do **not** route reports through A2UI — it would be a
competing pathway ("should I build a surface or save a file?"). A2UI = live UI; Work = produced
files. Names are close ("Work" vs "Workspace") but the surfaces are distinct and both already exist.
(Note: A2UI files live under `app/`, not the stale `app-v1/` some search hits referenced — verify
paths against `app/` when building.)

---

## 4. Frontend reuse map (CODE_AUDITOR rule 1)

| Need | Reuse | Location |
|---|---|---|
| Markdown render in pane | `marked` via existing `renderMarkdown()` | `ChatPane.svelte:6,14-18,109-113` — **replace** the line-by-line `startsWith('## ')` hack at ~951-969 |
| Artifact model + open | `interface Artifact`, `openArtifact()`, `activeArtifact` | `ChatPane.svelte:20-26,116(stub),119,121-126` |
| Inline cards under message | existing card markup + `openArtifact(id)` onclick | `ChatPane.svelte:761-774` (filters `a.messageId === msg.id`) |
| Pane tabs + content render | code=`<pre>`, table=`<table>`, doc=markdown | `ChatPane.svelte:896-970` |
| Icons | `artifactIcons` (lucide) | `ChatPane.svelte:118` |
| Populate from run | `controller.svelte.ts` handlers | `handleChatComplete` 232-265, `handleToolResult` 350-377 |
| `prose prose-sm` styling | `@tailwindcss/typography` | `app/src/app.css:8-9` |

The `GLP-1_Deep_Research_Report.md` "chip" is **not** a component today — it's inline-code in the
assistant markdown. Making it clickable means detecting file references and rendering them as the
existing inline artifact card (→ `openArtifact`).

---

## 5. SECURITY finding (independent bug — fix regardless of this feature)

`GET /api/v1/files/*path` (`crates/server/src/handlers/files.rs:146-181`) joins the user path onto
`<data_dir>/files/` with **no path-traversal guard** — `GET /api/v1/files/../../<anything>` resolves
outside the sandbox. Fix: `canonicalize()` and assert the result `starts_with(<data_dir>/files)`
before serving. (This must land before broadening the served root.)

---

## 6. Build plan (phased, each phase shippable + CODE_AUDITOR-clean)

**P0 — Security pre-req.** Add the path-traversal guard to `serve_file`. (Small, independent.)

**P1 — Emit produced files into the canonical pathway.**
Make file-producing tools surface the path via the existing artifact pathway instead of only
`.content` text:
- `bot(action:"deep_research")` → set `ToolResult.image_url` to the saved `report.md` path; the
  existing `chat_dispatch` copy-into-`files/` + normalize handles the rest. (Report already saved by
  `deep_research::finish()` → `<data_dir>/research/<run_id>/report.md`; either copy it into `files/`
  via the existing materializer, or broaden the served root + guard from P0.)
- Optionally the `system` file-write action when it produced a user-facing document.
- **Extend the artifact payload** from a bare URL to `{ url, title, kind }` (kind inferred from
  extension: md→document, csv/xlsx→table, png/jpg/svg→image, pdf→document, code→code). This is a
  *minimal extension of the existing pathway*, not a new one.

**P2 — Populate the pane + render.**
- Replace `const artifacts = []` with `$state`, populated in `controller.handleChatComplete` from
  the extended `data.artifacts` (`{url,title,kind}` + `messageId`).
- Lazy-fetch content on `openArtifact` via `GET /api/v1/files/<name>`; render with `renderMarkdown`
  (document), `<pre>` (code), table parse (csv), `<img>` (image).
- Make the inline cards + file chips clickable → `openArtifact`.

**P3 — (optional) persistence/inventory.**
Only if "list all Work across threads" is wanted: a `work_items(id, chat_id, message_id, path,
kind, title, created_at)` table (NOT `artifacts`). Defer until needed — per-message artifacts cover
the immediate UX.

---

## 7. Anti-checklist (don't)

- ❌ Create an `artifacts` DB table (collides with `artifact_update_prefs`). Use `work_items` if ever needed.
- ❌ Add a new "produced file" signal field — reuse `ToolResult.image_url` + the chat_dispatch pathway.
- ❌ Route reports through A2UI surfaces (competing pathway with the file/Work model).
- ❌ Re-implement markdown rendering — reuse `marked` / `renderMarkdown()`.
- ❌ Broaden the `/api/v1/files` served root without the P0 traversal guard.

## Key files
`crates/tools/src/registry.rs` (ToolResult.image_url) · `crates/server/src/chat_dispatch.rs`
(1159-1182 normalize/materialize, 483-494 collect, 845-851 broadcast) · `crates/server/src/handlers/files.rs`
(serve_file) · `crates/tools/src/deep_research.rs` (finish/persist) · `crates/tools/src/bot_tool.rs`
(handle_deep_research) · `app/src/lib/components/chat/ChatPane.svelte` (pane + Artifact scaffolding) ·
`app/src/lib/chat/controller.svelte.ts` (handleChatComplete/handleToolResult) ·
`app/src/lib/websocket/listeners.ts` · `app/src/app.css` (prose).
