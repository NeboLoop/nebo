# The Brief (Phase 1)

`brief.md` lives in the app folder, beside `ui/` and `src/`. About 40 lines.
Every section is required. Write it before any board, asset or code. The
gate reads it, and the next app on this bot reads its front-matter to stay
different.

## Template

```markdown
---
palette: celadon + vermilion on limestone
type: Cabinet Grotesk + IBM Plex Mono
hero_layout: image-as-canvas, text bottom-left
signature_effect: C3 scroll mask reveal
button_style: viewfinder brackets + route-line link
corner_shape: sharp, hairline rules
mode: generation
---

# <App name>

Design read: <who it is for, the emotional register, one sentence>
Concept spine: <one sentence the whole app pays off>
Delivery tier: cinema
Palette: #E4E8DF ground, #1F2A24 ink, #E2462F accent. <one-line defense>
Type: Cabinet Grotesk (display), IBM Plex Mono (labels, numbers). <why>
Animation mode: animated (owner picked Animated at intake)
Signature effect: C3 scroll mask reveal. <one sentence on how it enacts the spine>
Mobile: <how the effect degrades on a phone>

## Screens
1. <screen or section>: <layout family>, <the one focal visual>, <CTA if any>
2. ...

## Assets
- hero: <subject, framing, grade>
- plates: <2 to 3 backgrounds>
- icons: <set of N, stroke style>
- logo: <monogram idea, or "owner's own">
- video / glb / sound: <if the tier or effect needs them>

## CTA inventory
- <label>: <intent>, <its own garment and interaction>
- ...

## Previous apps
- <name>: <its six axes>; differs on <axes>
```

## Rules per line

**Front-matter axes.** Short labels, lowercase is fine. They are compared
with the other apps' briefs, so name the family, not a mood:
`palette: bottle green + bone`, not `palette: earthy`. `mode` is
`generation` or `no-generation` (see SKILL.md, Boards). Add
`brand_override: <reason>` only when the owner's own brand forces a banned
palette or face; the gate then reports those hits as warnings.

**Design read.** Who opens this app and what they should feel. "Busy
owners checking on a job between site visits; calm, certain" beats "users".

**Concept spine.** A nameable idea threading every screen: an instrument,
an archive, a stage, a journey with waypoints, a living system, a
collectible artifact. Derive it from the nouns of the owner's world (steel,
paper, steam, moss, vinyl, ledger). If the spine could describe any app, it
is not done.

**Delivery tier.**
- `cinema` (default): smooth scroll, a Tier-1 hero effect, chapters.
- `spectacle`: cinema plus 3D or WebGL, a second beat mid-page from another
  effect family, a custom cursor on desktop. For "immersive", "3D", "wow".
- `editorial`: type, imagery and bespoke chrome, micro-motion only. Only
  when the owner asked for calm or still, never as your own shortcut.

**Palette.** Exact hexes: a ground, an ink, exactly ONE accent (saturation
under about 80%), optional tints of the same family. One theme per app: a
dark app is dark on every screen. Defend the accent in one line from the
owner's material world. Banned families are in `design-recipe.md`.

**Type.** A display face and a text or mono face, named. Serif only with a
written reason (a real editorial, luxury or heritage institution, not "a
bakery"). Inter is never the display face.

**Animation mode.** The literal intake answer: `animated` or `still`, with
the reason when `still` ("owner picked Still at intake"). Your own taste is
never the reason.

**Signature effect.** One catalog ID from `wow-catalog.md` plus one
sentence on how it enacts the spine. For a game, the signature is the core
verb's feel (see `games.md`).

**Screens.** Ordered. One layout family per screen, no two in a row the
same, at least 4 families when there are 6 or more screens. Name the focal
visual of each.

**Assets.** Every asset the screens need, so Phase 3 submits them all at
once. Each line names subject, framing and grade.

**CTA inventory.** Every call to action, each with its own garment and
interaction (see the garment list in `boards-and-assets.md`). One label per
intent across the whole app.

**Previous apps.** Each other app on this bot and the axes this one differs
on. At least 4 of 6 each. First app on the bot: write "none; axes derived
from <material words>" and treat the model's own defaults as the thing to
differ from.

## Generic lines that mean "not done"

"modern and clean", "sleek", "minimal and elegant", "Inter", "blue
accent", "dark mode", "vibrant", "user-friendly", "engaging experience",
"cutting-edge". Replace each with a decision someone could disagree with.
