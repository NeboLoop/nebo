# Boards, Assets and Building to the Boards (Phases 2 to 4)

## Generating

Art and film come from one tool, `generate_media` (kind `image` or
`video`). It runs through Nebo's own inference (billed to the bot's plan),
waits on slow jobs, and writes the file where you say. Its options are flat
parameters:

```
generate_media(kind: "image", prompt: "...", into: "boards/hero.png")
generate_media(kind: "image", prompt: "...", size: "1536x1024", output_format: "webp", into: "assets/hero.webp")
generate_media(kind: "video", prompt: "...", seconds: 8, resolution: "720p", image: "assets/hero.webp", scrub: true, into: "assets/film.mp4")
```

- `into` is relative to the app's served folder (the folder `ui/` builds
  into), so `assets/film.mp4` is served as `assets/film.mp4`. The result
  names the file written. Pass `app` only when building another app.
- `scrub: true` re-encodes a film so every frame is a keyframe, which a
  scroll scrub needs (see `film-scrub.md`). Without ffmpeg on the bot the
  result says so and keeps the original.
- A long video that outlives the wait comes back with a job id; call again
  with `job: "<id>"` to collect it instead of paying for a new one.
- Sound has no generator yet: build effects in code (Web Audio; see
  `games.md`) or use the owner's files.
- Submit independent jobs together; build while they render.
- You see an image only by reading the file (the vision helper describes
  it). Never paste images into the conversation.
- If a job is refused or fails twice, change the prompt's structure (drop
  mood words like "intimate" or "ambient", describe the shot plainly) or
  the model option. Never fall back to stock. If something is genuinely
  unavailable, say so in the final report.
- `generate_media` is a deferred tool: load it with
  `find_tools(query: "select:generate_media")`. Only when that finds nothing
  are you in no-generation mode (see SKILL.md, Design Depth).

## Boards (Phase 2)

ONE landscape image (16:9 or 3:2) per screen or section, never one tall
full-page image (detail turns to mush and every screen ends up composed
alike). Six screens, six boards. Boards go in `boards/` (where `generate_media` writes them); delete that folder once the build matches them, the gate refuses to publish while it exists.

### The combination (commit before prompting)

Pick ONE option per row, write the picks into the brief, hold them across
every board.

- **Theme:** pristine light (paper, off-white, dark ink) / deep dark (most
  overused: needs a twist) / bold solid field (oxblood, royal blue, forest,
  vermilion) / quiet neutral (bone, sand, stone, smoke).
- **Background character:** technical grid or dot field / solid with soft
  depth / full-bleed cinematic image / tactile paper or material texture.
- **Type character:** clean grotesk / refined grotesk / expressive display /
  compressed statement / Swiss rational with hard hierarchy / editorial
  serif plus sans (with the brief's written reason).
- **Hero architecture:** image-first with restrained text / inline type
  behemoth / editorial offset / floating print scatter / masked reveal /
  asymmetric split (not left-text right-image).
- **Screen system:** bento rhythm / alternating editorial blocks / poster
  stack / gallery cadence / Swiss grid / asymmetric flow.
- **Four signature components:** staggered masonry, cascading card deck,
  hover-accordion slices, gapless bento, turning print arc, vertical rhythm
  lines, oversized metrics strip, layered crop frames, split testimonial
  wall, off-grid editorial blocks.
- **Second-read moment (exactly one, placed once):** asymmetric bleed, one
  oversized numeral or glyph as structure, one material switch, a narrow
  side-rail note, a macro crop in the accent.

Each board picks its own composition anchor (centered statement, top-left
lead, bottom-left over image, off-grid offset, image-as-canvas). At least 3
different anchors across the app. Background treatment varies per screen
too.

### Prompt recipe (per board)

"app screen design mockup, [phone portrait | desktop], [SCREEN ROLE],
[theme + the exact palette words and hexes], [type character] typography,
[composition anchor], [background treatment], [spine motif], the headline
'[the real headline you plan]', clear hierarchy and spacing, award-winning
interface design, no watermark, no browser chrome, no device frame".
Ask for the highest quality setting. Put the real copy in so type sits
believably.

### The redo rule (look at every board)

Read each board. Ask: would this hang on a studio's portfolio, or does it
read as a template? Redo any generic one (centered dark hero, glowing
gradient blob, card trio, dashboard spam, beige serif luxury) with a
stronger direction: push the anchor harder or change the background
treatment. Two redos per board; a board that fails twice means change the
combination pick, not the wording.

A board that cannot answer "what does the build copy from this?" is mood
art. Redo it.

## The Asset Kit (Phase 3)

Every prompt carries the locked hexes, the spine's material words, and "no
text, no logos, no watermark" (type is set in HTML). Downscale: hero at
most 2048px wide, cutouts about 800px, icons about 256px. WebP or AVIF for
images.

**Always:**
1. **Hero visual.** Two candidates, keep one. Consider an interaction pair:
   the same composition in two grades (dormant and lit) for reveal effects.
2. **Screen plates.** 2 or 3 backgrounds matching the boards (material
   macro, graded gradient, paper grain) so screens are never flat fills.
3. **Content imagery.** Every image the screens need, same grade. A product
   UI that does not exist is generated as an image, never built from divs.
4. **Icon set.** One sheet of 6 to 12 glyphs in one stroke style on a solid
   key color, sliced and keyed to transparency; or one glyph per job.
5. **Logo or monogram**, only when the owner has none. It also becomes the
   app's icon.

**Cinema tier adds** the film for the signature effect (see
`film-scrub.md`), or a loop for a mid-page band.

**Spectacle tier adds** a 3D subject (`.glb`) or shader material.

**Pick by fit from the brand ladder** (3 done coherently beat 8 scattered):
a logo family (mark, wordmark, monogram from one source), a seamless brand
pattern, a spot illustration set in one named style, state art (empty,
success, error, loading), consistent portraits, a product shown from
several angles, diagrams as art instead of boxes and arrows, a short brand
sound behind a tap-to-play toggle.

**Rules:**
- The owner's assets always win. If their photos fight the direction,
  offer a regrade, never a replacement.
- One coherence look at the whole kit together, once. Regenerate anything
  whose grade fights the boards, naming the hexes harder. A mixed-grade kit
  looks cheaper than no kit.
- Rejected candidates go to `refs/`. Everything in `ui/assets/` is
  referenced by the page (the gate checks).
- Each file at most 10 MB, the whole `ui/` at most 50 MB, or it will not
  publish. Video: shorten or lower the bitrate before anything else.

## Building to the Boards (Phase 4)

Reread the board when you build its screen; never code from memory.
Extract, per board:

- **Text:** wording, line count, wrapping, alignment.
- **Type:** size ratios between display, heading and body; weight contrast;
  tracking.
- **Spacing:** headline to sub-line, text to action, gaps, gutters, screen
  padding. Keep the board's spacing logic; never collapse generous space
  into default tight spacing.
- **Color:** ground, panels, accent placement, text colors, image grade.
- **Components:** button shape and fill, card structure, dividers, borders.
- **Rhythm:** repeated motifs (hairlines, numerals, crop frames, rail
  notes). These carry the spine.

**Anti-drift:** do not simplify a distinctive screen into a generic row, do
not flatten strong type into a default hierarchy, do not swap the board's
palette for tokens you are used to. After each screen, compare board and
code and name one thing you kept faithful.

**When a board is ambiguous**, in order: keep the visible design language,
then the layout and spacing logic, then the component family, then the
polish level; then generate a close-up of the unclear region; then redo
that board; only then take the easiest faithful reading. Never fill a gap
with a generic default first.

**What on a board is not binding:** a nav bar or footer drawn inside one
screen's board; eyebrows beyond the ration; generic buttons (the brief's
CTA inventory decides each action's garment).

### Bespoke chrome (gate-checked in spirit, reviewed by you)

No shared button style stamped across the app. Each action in the CTA
inventory is designed where it lives, with its own interaction. The overused
trio (drawing underline, hover flood-fill, framed block) is rationed: at
most ONE of them per app. Pick the rest from garments like these,
re-expressed in the brief's material world:

- a text link whose arrow travels along a drawn path (route, circuit, seam)
- a label that splits apart to reveal where it goes
- an action embedded in an image cutout (the button is a tag, a ticket)
- a stamp that imprints on press (skew plus texture shift)
- a ticket with a perforation that tears on hover
- a mono readout that decodes the label on hover
- a circular badge that spins; text on a path
- an underline that is a waveform, route or thread
- a swatch that flips like a material sample
- an oversized numeral as the hit area, the label as its caption
- a full-width band that shears or shifts grade on press
- viewfinder corner brackets that close around the label
- a toggle for binary choices (listen or read, day or night)

The test: cover the label. Could you still tell which app the button
belongs to? If it could live anywhere, it is not done.

### Structural hygiene

- No card inside a panel inside a frame unless the board shows it. Depth
  usually comes from one surface change.
- No badges, dots, chips or icon confetti the board does not show.
