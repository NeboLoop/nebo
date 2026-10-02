# The Gate (Phase 6)

```
execute(skill: "app-studio", script: "scripts/gate.js", args: { "app": "<app folder>" })
execute(skill: "app-studio", script: "scripts/gate.js", args: { "app": "<app folder>", "others": "<folder of the bot's other apps>" })
```

It needs node (or bun) on the bot and no packages; without one, check the
list below by hand. Without `others` it compares
the app with every sibling folder of the app folder that holds a
`brief.md`, which is where the bot's other apps live. Exit code 1 means at
least one failure: fix them all and run it again. A build with a failing
gate is not done.

## What fails the build

**The brief.** `brief.md` missing; a front-matter axis missing (`palette`,
`type`, `hero_layout`, `signature_effect`, `button_style`,
`corner_shape`) or `mode` not `generation` / `no-generation`; a required
line missing (Concept spine, Delivery tier, Palette, Type, Animation mode,
Screens, Assets, CTA inventory); a Palette line without hex values; a
generic line ("modern and clean", "sleek", "blue accent", ...); a template
`<placeholder>` left in; Inter as the display face.

**Too close to another app.** Same value as any other app on the bot on
more than 2 of the 6 axes.

**The package.** AGENT.md without `artifact_type: app`; any AGENT.md
frontmatter value containing `: ` without quotes (the marketplace would
silently make a plain employee); manifest.json without `"type": "app"`;
an em-dash in either description.

**The page.**
- `ui/index.html` missing, or no viewport meta tag.
- A file over 10 MB, `ui/` over 50 MB, or a file type that cannot publish.
- Em-dashes or en-dashes in visible text (comments are ignored).
- Elevate, Seamless, Unleash, Next-Gen, Revolutionize.
- Placeholders: lorem ipsum, TODO, FIXME, picsum, placeholder services,
  empty `src=""`.
- A banned palette hex (warning instead when the brief records
  `brand_override`).
- `h-screen`.
- Animation with no `prefers-reduced-motion` handling anywhere.
- No `touch-action` anywhere.
- A bare `nebo` global, or the SDK used without loading
  `../../../sdk/nebo.global.js`.
- A script, style or image in `ui/index.html` whose path starts with `/`
  (it breaks on the phone).
- `package.json`, `node_modules/` or `src/` inside `ui/`; an `index.html`
  in the app folder itself (only `ui/index.html` and a build's
  `src/index.html` may exist).
- Video without `playsinline` or `muted`.
- A file in `ui/` nothing references.
- `mode: generation` with no generated image, film or model in `ui/`; a
  film-scrub signature (catalog A) with no film; `animated` with nothing
  animating.

## Warnings (check by eye)

`100vh`; a three-column grid (make sure it is not three equal cards); a
reveal that may wait at opacity 0 for a scroll trigger; Inter loaded at
all; no canvas, SVG or shader art in no-generation mode; dashes in the
brief.

## What the script cannot see (check these yourself)

- A fake product UI built from divs in the hero.
- One label per intent across the app; primary labels 3 words or fewer.
- Each action has its own garment; at most one of the rationed trio.
- Eyebrows at most one per three screens.
- The screens match the brief's screen plan and the boards.
- Every visible string reread: nothing vague, broken or invented.
- The app running: `app_reload`, then `app_console` shows no errors, and
  `app_screenshot` at phone and desktop size matches the boards.
