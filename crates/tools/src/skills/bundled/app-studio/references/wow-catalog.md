# Wow Catalog (pick in Phase 1, build in Phase 5)

The signature effect is the thing a person remembers. ONE per app, chosen
in the brief from this catalog (not from the first idea that comes to
mind), and fully executed. The wow is generated media responding to the
person's input, not a CSS trick.

## Selection rules

1. **Pay off the spine.** The effect enacts the app's idea: an "instrument"
   spine wants focus and precision; an "ascent" spine wants climbing depth;
   an "archive" spine wants leafing and stacking. One sentence in the brief
   defends the pairing.
2. **Never repeat the previous app's effect** (see the ledger below). If the
   spine truly demands it, change the subject, the axis or the payoff.
3. **Cinema:** one Tier-1 plus motivated reveals. **Spectacle:** one Tier-1
   plus a second beat mid-app from a different family, plus a custom cursor
   on desktop. **Editorial:** a light D-family effect or none.
4. **Screenshot-safe** (the first frame is a finished composition before any
   input) and **reduced-motion-safe** (a composed static fallback).
5. **Phone first.** Declare in the brief how it degrades: shorter pins,
   pointer effects replaced by scroll or tilt equivalents, a turntable that
   becomes a swipe.

## The catalog

### A. Film scrub (scroll plays a generated video)

- **A1 Single-shot scrub.** One continuous generated take (push-in, rack
  focus, subject turn, light sweep; the start state differs from the end
  state) scrubbed by scroll from the first pixel to the last. The proven
  baseline; ready template in `film-scrub.md`.
- **A2 Chaptered scrub.** 2 to 4 takes in one grade (wide, detail, reveal)
  across a long scroll, with pinned text cards, a progress rail or a
  readout handing off between chapters. Budget it only when nothing else
  on the page is heavy.
- **A3 Product turntable.** Several generated angles of the same product,
  or a short orbit film: scrolling rotates the product.

### B. Layered depth (one image becomes a scene)

- **B1 Cutout parallax rig.** The hero subject cut out, a clean background
  plate behind it, a mid layer (fog, foliage, particles) as transparent
  images. 3 to 5 layers moving at different rates on scroll, and on tilt or
  pointer.
- **B2 Grade-shift pair.** Two grades of the same composition (dormant and
  lit), revealed by a pointer spotlight mask or by scroll. "The app notices
  you."
- **B3 3D subject.** A `.glb` in a three.js scene with a scroll-driven
  camera orbit and pointer or tilt. Spectacle tier.

### C. Canvas and pixels (the image itself is alive)

- **C1 Displacement hover.** The hero on a WebGL plane; the pointer or a
  touch ripples it (three or ogl).
- **C2 Particle dissolve.** The hero sampled into particles that assemble on
  load and scatter or reform with scroll or pointer.
- **C3 Scroll mask reveal.** The app opens inside giant type or a shape;
  scrolling grows the mask until the media is full-bleed (`clip-path` or
  canvas).

### D. Spatial (the page itself moves unusually)

- **D1 Horizontal rail.** A pinned screen pans sideways through a wide
  generated panorama or a sequence of plates while content rides the rail.
  On phones, keep the gesture vertical (scroll drives the pan) so the page
  never pans sideways under the finger.
- **D2 Sticky stack.** Full-bleed chapters stack and peel over each other,
  each with its own plate.
- **D3 Kinetic type.** Massive display type choreographed by scroll
  (per-character stagger, variable-font axis animation, lines on separate
  tracks) over a generated plate. When the brand voice is the type.

### Games

The signature is the core verb's feel: input to response latency, hit
pause, screen shake as data, particles on impact, a camera that leads the
motion. See `games.md`.

**Banned as Tier-1:** an autoplay loop on its own, fade-in reveals,
particles behind text with no interaction, marquee strips, tilt-on-hover
cards. They may exist as seasoning, never as the answer to "what is the
wow".

## Implementation contracts (every family)

- First paint is complete: frame 1, the layer stack or the unmasked state
  renders before any script-driven interaction.
- Response feels physical: scroll-linked progress with a short smoothing
  (`scrub: 0.5` to `1` in ScrollTrigger), pointer through springs, written
  straight to transforms. Never a state update per frame, never a layout
  property.
- Reduced motion: the composed final state, static, no pin.
- Smooth scroll with Lenis must be bridged to the GSAP ticker (see
  `kit.md`), or scrubs stutter.
- A pinned section adds a spacer; check the screenshot at each scroll stop
  has no empty band after it (`pinSpacing: false` with the next content
  sliding over, when that reads better).

## Anti-convergence ledger (the gate checks it)

Each app is compared with every other app on the bot on six axes, read from
the front-matter of each `brief.md`:

1. `palette` (family)
2. `type` (the display face)
3. `hero_layout`
4. `signature_effect` (catalog ID)
5. `button_style` (the garment set)
6. `corner_shape` (sharp, soft, pill, hairline)

The new app must differ from each on at least 4. The first app on a bot
derives all six from the owner's material world and says so; the enemy is
then the model's own statistical default. If an axis choice would look at
home in a generic template, choose again.
