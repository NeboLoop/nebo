# Film Scrub (catalog A1, the ready template)

The person's scroll plays a generated film forward and backward, and holds
any frame as a still when they stop. Chapter copy reads over it. This
template was proven on iPhone inside the Nebo app view (hundreds of
coalesced seeks, both directions) and on desktop. Use it as written; change
the look, not the mechanics.

## The eight rules (each one learned the hard way)

1. **Encode the film all-keyframe** (`-g 1`, `+faststart`). A normal MP4
   seeks backward in jumps and a scrubbed film stutters.
2. **Mark it `muted playsinline`.** Without them the phone opens its own
   full-screen player.
3. **`load()`, then prime with one muted `play()` and `pause()` before the
   first seek.** WebKit will not seek a video that has never played, and
   iOS ignores `preload`.
4. **Coalesce seeks.** Start a new seek only after the previous `seeked`
   fires, always toward the newest target. Fast scrolling then never builds
   a backlog of stale seeks.
5. **Read progress from the real scroller** (`document.scrollingElement`)
   and listen for `scroll` on `document` in the capture phase (plus
   `window`). Then it works whichever element actually scrolls.
6. **Move from the first pixel of scroll.** People stop scrolling when
   nothing answers.
7. **Hand the film to the player while it plays.** If the person presses
   play, the scrub stops writing `currentTime` until it is paused again.
8. **Vertical panning only:** `touch-action: pan-y; overscroll-behavior-x:
   none`, and nothing wider than the screen. One long unbroken string once
   panned the whole page sideways.

## The footage (direct it for scrubbing)

- **One continuous move, no cuts.** A slow orbit, push-in, rise or
  fly-through, or one continuous transformation of the subject.
- **One subject, center-safe,** with clean space around it for the copy.
  The film fills the screen with `object-fit: cover`, so a phone in
  portrait crops the sides.
- **A background copy can survive:** dark, seamless, low detail. A bright
  busy frame behind body text is the most common reason a beautiful film is
  unusable.
- **Slow and steady:** constant speed; the scroll supplies the pacing.
- **Locked exposure and white balance, minimal motion blur:** every frame
  is shown as a still.
- **Start differs from end:** the first frame establishes, the last is the
  payoff.
- **No text, logos or watermarks in the film.**

Make it with `generate_media(kind: "video", ..., scrub: true)`,
which encodes it for scrubbing. Generate a 6-keyframe storyboard image
first (one continuous move laid out as a 6-panel grid, "not six different
scenes") and pass it as a style reference, not as the first frame. The
longest single take the model offers (about 8 to 15 s), audio off.

## Encoding by hand (when you have the raw file and ffmpeg)

```bash
# Desktop: native size, every frame a keyframe, no audio, fast start
ffmpeg -y -i raw.mp4 -an -c:v libx264 -preset slow -crf 22 -pix_fmt yuv420p \
  -g 1 -keyint_min 1 -sc_threshold 0 -movflags +faststart ui/assets/film.mp4
# Phone: at most 720p tall
ffmpeg -y -i raw.mp4 -an -vf "scale=-2:'min(720,ih)'" -c:v libx264 -preset slow -crf 24 \
  -pix_fmt yuv420p -g 1 -keyint_min 1 -sc_threshold 0 -movflags +faststart ui/assets/film-720.mp4
# Poster: the first frame of the exact file the page plays
ffmpeg -y -ss 0 -i ui/assets/film.mp4 -frames:v 1 -q:v 2 ui/assets/film-poster.jpg
```

All-keyframe files are larger. Keep each under 10 MB (the publish limit per
file): shorten the take, lower resolution, or raise CRF before anything
else. Under about 40 MB total for all films.

## The template

`ui/index.html` (the parts that matter):

```html
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
...
<section id="scrub" class="scrub">
  <div class="scrub-stage">
    <video id="film" class="scrub-film" muted playsinline preload="auto"
           poster="assets/film-poster.jpg"
           data-src="assets/film.mp4" data-src-phone="assets/film-720.mp4"></video>
  </div>
  <article class="chapter"><h2>First chapter headline</h2><p>One sentence.</p></article>
  <article class="chapter"><h2>Second chapter</h2><p>One sentence.</p></article>
  <article class="chapter"><h2>The payoff</h2><p>One sentence.</p></article>
</section>
```

`ui/style.css` (layout contract only; the look comes from the brief):

```css
html, body { touch-action: pan-y; overscroll-behavior-x: none; overflow-x: hidden; }
.scrub { position: relative; }
.scrub-stage { position: sticky; top: 0; height: 100dvh; overflow: hidden; }
.scrub-film { width: 100%; height: 100%; object-fit: cover; display: block; }
.chapter {
  position: relative; z-index: 1; min-height: 100dvh; margin-top: -100dvh; /* first one overlays the stage */
  display: grid; align-content: end;
  padding: 0 1.25rem max(2.5rem, env(safe-area-inset-bottom));
  overflow-wrap: anywhere;
}
.chapter + .chapter { margin-top: 0; }
@media (prefers-reduced-motion: reduce) { .scrub-stage { position: relative; } }
```

`ui/scrub.js`:

```js
// Film scrub: scroll position drives the film's time.
const film = document.getElementById('film');
const section = document.getElementById('scrub');
const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;

// The source is set here, never in the markup, so reduced motion fetches
// nothing: the poster is the still.
if (!reduce) {
  const phone = matchMedia('(max-width: 820px), (pointer: coarse)').matches;
  film.src = phone ? film.dataset.srcPhone : film.dataset.src;
  start();
}

async function start() {
  film.muted = true;              // rule 2 (also in the markup)
  film.load();                    // rule 3
  if (film.readyState < 1) {
    await new Promise((res) => film.addEventListener('loadedmetadata', res, { once: true }));
  }
  await prime();
  // Prime again on the first touch, in case the first try was refused.
  addEventListener('pointerdown', prime, { once: true, passive: true });

  let target = 0;
  let seeking = false;

  // rule 6: from the first pixel of scroll to the bottom of the section.
  const progress = () => {
    const se = document.scrollingElement || document.documentElement;
    const end = section.offsetTop + section.offsetHeight - innerHeight;
    return Math.min(1, Math.max(0, se.scrollTop / (end > 0 ? end : 1)));
  };

  // rule 4: one seek in flight, always toward the newest target.
  const seek = () => {
    if (seeking || !film.duration || !film.paused) return; // rule 7
    if (Math.abs(film.currentTime - target) < 1 / 48) return;
    seeking = true;
    film.currentTime = target;
  };
  film.addEventListener('seeked', () => { seeking = false; seek(); });

  // rule 5: the real scroller, capture phase, one update per frame.
  let queued = false;
  const onScroll = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      target = progress() * (film.duration || 0);
      seek();
    });
  };
  document.addEventListener('scroll', onScroll, { capture: true, passive: true });
  addEventListener('scroll', onScroll, { passive: true });
  addEventListener('resize', onScroll, { passive: true });
  onScroll();
}

async function prime() {
  // rule 3: WebKit will not seek a video that has never played.
  try { await film.play(); film.pause(); } catch (_) { /* first touch retries */ }
}
```

Load it at the end of `<body>`: `<script src="scrub.js"></script>`.

## Chapter motion

Chapter copy stays in normal document flow and fully rendered. Move it with
transforms only (a short rise, a clip-path wipe), never from `opacity: 0`
waiting on a viewport trigger. If you add Lenis and GSAP for the rest of
the page, do not attach a second timeline to the film: this controller owns
its time.

## When there is no encoder: WebP sequence on a canvas

If `generate_media` reports the film was not re-encoded for scrubbing
(no ffmpeg on the bot), draw it as a frame sequence instead (split one
where ffmpeg exists: `ffmpeg -i raw.mp4 -vf "fps=24,scale=1280:-2" ui/assets/frames/%04d.webp`),
draw frames on a canvas instead:

```js
const canvas = document.getElementById('film-canvas');
const ctx = canvas.getContext('2d');
const N = 120; // frame count
const frames = new Array(N);
const load = (i) => frames[i] || (frames[i] = Object.assign(new Image(), {
  src: `assets/frames/${String(i + 1).padStart(4, '0')}.webp`,
}));
const draw = (i) => {
  // Nearest loaded frame while the rest stream in.
  let f = load(i);
  for (let d = 1; !f.complete && d < N; d++) f = frames[i - d] || frames[i + d] || f;
  if (!f.complete) return;
  const s = Math.max(canvas.width / f.naturalWidth, canvas.height / f.naturalHeight);
  const w = f.naturalWidth * s, h = f.naturalHeight * s;
  ctx.drawImage(f, (canvas.width - w) / 2, (canvas.height - h) / 2, w, h);
};
load(0).onload = () => draw(0);         // first frame painted at once
for (let i = 1; i < N; i++) load(i);    // stream the rest
// In the scroll handler: draw(Math.round(progress() * (N - 1)));
```

Size the canvas to `innerWidth * devicePixelRatio` (cap the ratio at 2) on
load and on width changes.

## Checks before the gate

- Scroll slowly and fast, both directions, on a phone size and a desktop
  size (`app_screenshot` at several scroll positions; `app_console` shows
  errors from the owner's open view).
- The poster shows before the film loads; no black box at any point.
- Reduced motion shows every chapter over the poster and fetches no film.
- Nothing pans sideways on a phone.
