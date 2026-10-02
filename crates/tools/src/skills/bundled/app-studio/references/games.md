# Games

A game runs the same flow (intake, brief, boards, assets, build, motion,
gate). Read this whole file before the brief. Planning is done in your head
except for two written things: the game part of `brief.md` and the asset
manifest. No game code before both exist.

## 1. The game brief (add to `brief.md`)

**Profile.** Place the game on each axis:

| Axis | Range |
|------|-------|
| Time | real-time / turn-based / pause-at-will |
| Space | continuous 2D or 3D / grid / abstract |
| Agency | one hero / a squad / a hand from above |
| Conflict | the system / other players / yourself |
| Content | authored / procedural / emergent |
| Outcome | win or lose / endless / own goals |
| Players | solo / co-op / versus |
| Session | a minute / ten minutes / once a day |
| Engagement | execution, calculation, discovery, expression, story, collection (pick 1 or 2) |

**Delivery:** phone and desktop by default. Every action works by touch
and by keyboard (physical key codes, never typed letters), and by gamepad
when declared. No hover-only interactions. Budgets target the weakest
device.

**Experience formula** (the compass, one sentence): "The player feels ___
because the game constantly ___." No genre labels.

**Core loop:** the 2 to 4 strong verbs (a strong verb gets different
answers from different things), what each does now, and where it echoes
later. A verb with no visible effect or no echo is dead.

**Style formula** (60 to 90 words, see section 3) and **asset manifest**
(section 4).

## 2. Laws

- **Experience first.** Any convention may break if the experience asks.
- **Every action is seen now and echoes later.**
- **Mastery:** teach one pattern at a time: introduce it safely, test it
  with a small price, combine it with what is known, then examine it under
  pressure. Every tutorial text is a scene that failed; build the situation
  that makes the player guess.
- **Uncertain outcome at every scale** (a move, a fight, a session): 2 or 3
  sources (execution, hidden information, randomness before the decision,
  another mind). When one runs dry another must already be active.
- **Loops:** a lead must not decide the game too early, and a comeback must
  exist, yet good play still wins.
- **Economy:** every resource has a source and a sink.
- **Entry:** a short, counted path from opening to the first meaningful
  action. On return, the first screen shows the goal and the next step.
- **Forgiving input:** generous timing windows; the player's hitbox smaller
  than its sprite, enemies' honest. Every action gets an instant response.
- **Numbers before code:** agency numbers (jump length, move range, speed)
  are fixed before content is built against them. Check every demand the
  content makes against them (a gap wider than the jump is a silent dead
  end).
- **Honest limits:** feel, music and real delight need human hands. Say so
  at delivery and ask the owner to play it.

## 3. The style formula

Every asset is a separate generation with no memory of the others. The
style formula is the one piece of context that travels into EVERY prompt,
**byte for byte, never paraphrased**, including single redos. Changing it
means redoing every asset, so only the owner's request to change the style
does that.

Five blocks, in order, 60 to 90 words:

1. **Rendering:** what it is drawn with ("flat vector with soft gradients",
   "chunky pixel art on a 32 px grid", "hand-painted gouache").
2. **Shape and line:** silhouettes and outlines ("rounded shapes, thick
   dark-plum outlines").
3. **Palette by role:** the environment's colors, a hero color that
   CONTRASTS with them, one signal hue for hazards and pickups. Not one flat
   list.
4. **Light and mood:** one clause.
5. **Readability and perspective:** "clean readable silhouettes, consistent
   side-view perspective". The perspective word matches the genre:
   side-view (platformer, runner), top-down, flat frontal (puzzle). A
   top-down game with a side-view hero is the most common failure.

Prompt assembly for every 2D asset:
`<kind template> + <asset description> + <style formula, exact> + <kind suffix>`

| Kind | Template | Suffix |
|------|----------|--------|
| sprite | `game sprite of <desc>, single subject, full body visible, centered,` | `, on a solid uniform bright <KEY> background, no shadow on the background, nothing cropped` |
| tile | `seamless tileable game texture of <desc>, even pattern density,` | `, seamless edges both ways, no border, no vignette, flat even light` |
| background | `game background of <desc>, wide establishing view,` | `, no characters, no UI, slightly muted detail so the play layer reads` |
| ui | `game UI element: <desc>, single element, centered,` | `, on a solid uniform bright <KEY> background, crisp edges` |

**Key color** (models cannot make transparency): magenta `#FF00FF` by
default; bright green `#00FF00` if the asset or formula has pink, magenta
or purple; bright blue `#0000FF` if both are taken. Key it out locally and
also clear enclosed key-colored regions (a donut hole is not connected to
the edge).

Sizes in the game: sprites 128 px, tiles 256 px, backgrounds 1280x720.
Pixel art scales with nearest-neighbor only. Two attempts per asset, then
take the best and compensate in code (tint, scale). Before wiring, put all
sprites side by side at their relative scales on a tile and look once: does
the hero pop, do proportions read?

## 4. The asset manifest

`design/assets.csv` in the app folder (planning file, never ships), one row
per asset: `id, role, type, description, size, source` (`generate` or the
owner's file). It is the contract between generation and the code; every
row must exist in `ui/assets/` and be used. Keep small games small: up to
about 10 visual assets, up to 2 music loops, about 5 sound effects.

## 5. Animation

**2D sprites from video.** An image model drawing frames one by one cannot
keep a character identical; a video model animates ONE image, so every
frame is the same character. Generate a key pose (the action's peak, full
body, empty margin above and below, on the key color), then
`generate_media(kind: "video", image: "<key pose file>", seconds: 4, into: "assets/...")`.
Loops pass the same image as first and last frame; one-shot actions
(attack, death) pass only the first. Then sample frames locally:

```bash
ffmpeg -i walk.mp4 -vf "fps=12" refs/walk/%03d.png   # then pick ~8-16 evenly, keep first and last
```

Key out the background per frame (never on the video), pack a grid sheet
(`ui/assets/hero-walk-8x1.png`), drop the duplicate last frame of a loop.

**Procedural animation** (no rig, or a creature no rig fits): sums of sines
per part, `angle(t) = A * sin(2 * PI * t / T + phase)`.
- Winged: wings 35 to 45 degrees, outer part lags inner by 0.15 to 0.25 of
  the cycle; the body bobs against the downstroke.
- Four legs: diagonal pairs in opposite phase.
- Six legs: alternating tripods.
- Snake or fish: a traveling wave, phase `i * 2 * PI / N` per segment,
  amplitude growing toward the tail.
- Blob: squash and stretch that keeps volume, `sy = 1 + A sin(wt)`,
  `sx = 1 / sqrt(sy)`.
- Idle for anything: breathing (2 to 4 degrees, a 4 s cycle) and a slow
  sway.

**3D models.** Load `.glb` with `GLTFLoader`; play clips with
`AnimationMixer` (`mixer.clipAction(THREE.AnimationClip.findByName(gltf.animations, 'Idle'))`,
cross-fade with `next.reset().fadeIn(0.25).play(); prev.fadeOut(0.25)`).
A material with `alphaMode: BLEND` on an opaque mesh looks like inverted
normals; set it opaque. Free CC0 sources when nothing is generated: Kenney
(kenney.nl), Quaternius, KayKit. Keep each file under 10 MB.

## 6. Audio

- Music: at most 2 loops. Effects: the main verb, damage or feedback,
  pickup, one environment sound, one ambience. Voice: one voice per
  speaker, short lines.
- There is no audio generator yet. Build effects in code with Web Audio
  (oscillators, noise, envelopes), or wire in files the owner provides.
- Normalize before wiring: voice about -6 dBFS, effects -10 to -12, music
  -18 to -20, true peak at most -3. Voice over effects over music.
- Use `mp3` or `m4a` (iPhone does not play `ogg`). Loops fade at the seam.
- The game stays playable with sound off; pause sound when the page is
  hidden.

## 7. The game template

Load it as a module (`<script type="module" src="./game.js">`) after the SDK
script (`../../../sdk/nebo.global.js`); `update(dt)` and `render(alpha)` are
yours. Put every tuning number in one exported `config` object.

```js
const { nebo } = window.NeboAppSDK;
const STEP = 1 / 60;                       // fixed-timestep simulation
let acc = 0, last = performance.now(), state = await load();

async function load() {
  return (await nebo.storage.getItem('save')) || { level: 1, best: 0 };
}
async function save() { await nebo.storage.setItem('save', state); }

// One first tap unlocks sound and (on iPhone) tilt.
let audio;
addEventListener('pointerdown', async () => {
  audio = new AudioContext(); await audio.resume();
  if (typeof DeviceMotionEvent !== 'undefined' && DeviceMotionEvent.requestPermission) {
    try { await DeviceMotionEvent.requestPermission(); } catch (_) {}
  }
}, { once: true });

// Input as commands, from physical keys and touch alike.
const held = new Set();
let tilt = 0;
addEventListener('keydown', (e) => held.add(e.code));     // e.code, never e.key
addEventListener('keyup', (e) => held.delete(e.code));
addEventListener('deviceorientation', (e) => { tilt = (e.gamma || 0) / 45; });
addEventListener('blur', () => held.clear());              // lost focus mid-action

function frame(now) {
  acc += Math.min(0.25, (now - last) / 1000); last = now;
  while (acc >= STEP) { update(STEP); acc -= STEP; }      // logic
  render(acc / STEP);                                      // drawing, separate
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
document.addEventListener('visibilitychange', () => { if (document.hidden) save(); });
```

- Logic separate from drawing; a seeded random generator so a bug
  reproduces from its inputs.
- Performance is a budget set before code (60 fps on a mid phone): one draw
  call per swarm of the same thing, nothing hidden is drawn, no allocations
  inside the frame loop.
- A debug overlay (fps, frame time, entity count) behind `?debug=1`.
- Save on every meaningful change and when the page hides; reopening on
  desktop or phone restores it.
- Manifest for a fullscreen landscape game: `window: { fullscreen: true,
  orientation: "landscape" }`, plus `device:motion` in permissions when it
  reads tilt.
- The play area: `touch-action: none` and `user-select: none`; pull to
  refresh is already off for fullscreen apps.

## 8. Multiplayer

The page opens a WebSocket straight to the game server; nothing goes
through Nebo. Rooms, presence and state relay live on the server. Send
input as commands with the player id, keep the simulation deterministic so
the server can arbitrate, and resend state on reconnect. Players are named
from `nebo.identity.get()`. Until the shared game server exists, build
solo or same-device play first and say so.
