# Bug: install reports "Installed" when the artifact was quarantined (signature verification failed)

**Date:** June 6, 2026
**Severity:** High (silent failure — user is told it worked when it didn't; trust-critical for a non-technical ICP)
**Component:** signing / signature verification (reads `signatures.json`); store install path (`POST /api/v1/store/products/<id>/install`); WebSocket tool-lifecycle events (`tool_quarantined`, `tool_error`); frontend marketplace install button/modal (shows success regardless of quarantine)
**Status:** Logged, not yet fixed
**Found via:** GUI install test — see `docs/testing/install-experience-2026-06-06.md`

## Symptom

Installing an agent from the marketplace shows a green **"Installed"** state (and the artifact
appears in `/marketplace/installed`), but the backend **quarantined** the artifact during install.
It does **not** become usable (initially absent from the Agents rail), and **no error is surfaced
to the user anywhere in the UI.** The user believes the install succeeded.

Reproduced installing agent `Research Report` (`AGNT-SW4Z-5XKN`, id `a726c13b-ee50-4d99-8d65-39b00ca850d4`).

## Evidence (console / WebSocket, captured during install)

```
WS  type: tool_quarantined
    data.toolId: a726c13b-ee50-4d99-8d65-39b00ca850d4
    data.reason: "signature verification failed: signing error:
                  read signatures.json: No such file or directory (os error 2)"

WS  type: tool_error
    data.error: "signing error: signature verification failed: signing error:
                 read signatures.json: No such file or directory (os error 2)"
```

The UI, meanwhile, flipped the Install button to **"Installed"** and rendered the success state.

## Root cause (two layers)

1. **Likely dev-env trigger:** `signatures.json` is missing locally, so signature verification
   fails and the artifact is quarantined. This specific trigger is probably local-build-only and
   may not reproduce in a signed production build.
2. **Product-level UX defect (the real bug):** the install flow treats server acceptance of the
   install as success and **does not listen for / react to** the `tool_quarantined` and
   `tool_error` WebSocket events for the artifact it just installed. So a quarantine — a *failed*,
   security-relevant outcome — is presented to the user as a *successful* install.

This also contradicts the homepage trust promise: *"Every solution is scanned and signed…
before it reaches you."* Here the scan failed and the user was told the opposite.

## Recommendation — make failures as loud as successes

The install experience is excellent precisely because it's frictionless; that makes a *silent*
failure the single most damaging defect, because a trusting non-technical user can't tell a real
failure from success. The fix is not to add friction — it's to make the unhappy path honest.

1. **The install UI must subscribe to the artifact's `tool_quarantined` / `tool_error` events**
   (keyed by `toolId`) for the thing it just installed, and resolve the modal/button state from the
   *actual* outcome, not from the install request returning 200.
2. **On quarantine/sig-fail, show a clear failure state**, not "Installed." Plain-language, e.g.:
   *"Couldn't verify this safely — not installed. This item failed our security check."* with a
   Retry and a "what happened" link. Never leave it reading "Installed."
3. **Quarantined ⇒ not "Installed."** Don't list a quarantined artifact under `/marketplace/installed`
   as if it's ready, and don't show its tile as installed in the Agents rail. If it must appear,
   badge it **Quarantined / Needs attention**.
4. **Principle to apply platform-wide:** every place the happy path shows a green confirmation, the
   corresponding failure must be equally visible and equally specific. Audit install, configure,
   connect/OAuth, and scheduled-task flows for silent-fail paths (server error, quarantine, 502,
   timeout) that currently resolve to a success or no-op state.

## Notes

- Severity is about the **UX contract**, not the sig-check itself: even once `signatures.json` is
  present in real builds, any future verification/quarantine failure must surface, or this class of
  silent failure recurs.
- Related observed-but-separate: the Agents rail wasn't realtime (installed agent missing until
  reload) — **fixed during the same session**; tracked in the test report, not part of this bug.
