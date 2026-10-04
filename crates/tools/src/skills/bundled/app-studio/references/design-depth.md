# Design Depth (the studio method)

For a game, a showcase, a landing app or anything the owner will show other
people, run these phases on top of Steps 0 to 4. A plain internal tool (a
tracker, a form) skips this section. Read each reference with
`read_skill_file(name: "app-studio", path: "references/<file>")` when its
phase starts, not before. A small edit to an app that already went through
the method (copy, one component, a color) does not restart it: edit,
rebuild, Verify, gate.

| # | Phase | Leaves | Reference |
|---|-------|--------|-----------|
| 0 | Intake: ONE batched question round (app or game; animated or still, recommend animated; their brand or free rein). No answer: animated, free rein, say so in a line. | the answers | |
| 1 | Brief: `brief.md` in the app folder, six variety axes in front-matter, concept spine, locked palette (hex) and type pair, screen, asset and CTA plans. Differs from every other app's brief on 4 of 6 axes. | `brief.md` | `brief.md`, `design-recipe.md`, `wow-catalog.md` |
| 2 | Boards: one generated image per screen into `boards/`, each looked at once, template-looking ones redone (two redos max). | `ui/boards/*.png` | `boards-and-assets.md` |
| 3 | Assets: every image, film and model submitted at once with the locked hexes; owner's assets win. | `ui/assets/*` | `boards-and-assets.md`, `games.md` for a game |
| 4 | Build each screen to its board (Step 3A or 3B). The board wins over habit. | the page | `design-recipe.md`, `kit.md` |
| 5 | Motion: ONE signature effect that answers the person's input, fully wired, with a `prefers-reduced-motion` fallback. | the effect | `wow-catalog.md`, `film-scrub.md` |
| 6 | Gate: zero failures, then Verify. Delete `ui/boards/` before publish. | a pass | `gate.md` |

**The gate.** On a bot with node, run this skill's checker on the app folder:
`execute(skill: "app-studio", script: "scripts/gate.js", args: { "app": "<app folder>" })`.
It fails on the brief, banned palettes and words, em-dashes, placeholders,
unused or oversize files, missing reduced-motion or touch handling, a
leading `/` in the page, project files inside `ui/`, an `index.html` in the
app folder, the package files, and closeness to the bot's other apps.
Without node, or when it cannot run, check the list in `references/gate.md`
by hand. A design-depth app with a failing gate is not done.

**Banned defaults** (the model's own habits): near-black plus orange, amber,
or neon cyan, blue or green; purple glow; beige plus brass, clay or
oxblood (unless the owner's brand names them); Inter as the display face;
three equal cards in a row; a fake product UI built from divs; em-dashes in
visible text; Elevate, Seamless, Unleash, Next-Gen, Revolutionize; invented
stats; "Jane Doe" testimonials; fade-ins and marquees as the signature
effect.

**No art tool.** Load `generate_media` with `find_tools` first. Only when
`find_tools` does not find it, write each board in words inside the brief,
record `mode: no-generation`, and make the art by hand (SVG, canvas, shaders).

**Turn economy.** Write each file once, complete. Submit independent
generations together. Look at each generated image once.

**Talking to the owner.** Product words: "Designing the screens", "Making
the art", "Your app is ready, open it from your workforce". Never narrate
bundlers, hashes or folders unless asked. At the end, list what the owner
now owns (logo, icons, art, film) and anything honestly skipped.

The method's design parts are adapted from an MIT-licensed work; the notice
is in this skill's `LICENSE-THIRD-PARTY.txt`.
