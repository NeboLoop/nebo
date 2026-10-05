# The Kit (Phases 4 and 5)

Free, permissively licensed libraries only. Never add a paid or proprietary
source. Lane A (no build, SKILL.md Step 3A) fetches them from esm.sh once,
when the app is written, and saves them into the app's `ui/vendor/`; Lane B
(Vite, Step 3B) installs and bundles them.

| Library | License | Use it for |
|---------|---------|-----------|
| `gsap` (with ScrollTrigger) | GSAP standard license, free incl. plugins | scroll-driven timelines, pins, scrubs, sequencing |
| `lenis` | MIT | weighted smooth scroll (bridged to the GSAP ticker) |
| `motion` | MIT | UI springs, enter and exit, layout animation, gestures |
| `split-type` | MIT | split headlines into lines, words, characters |
| `three` | MIT | 3D scenes, `.glb` models, particles, shaders |
| `@react-three/fiber` + `@react-three/drei` | MIT | 3D in React |
| `@react-three/postprocessing` + `postprocessing` | MIT / Zlib | bloom, depth of field, grain |
| `ogl` | Unlicense | a light custom shader without three's weight |
| `tailwindcss` v4 | MIT | utility styling, tokens from the brief |
| `maath` | MIT | easing and math helpers for 3D |
| `@number-flow/react` or `number-flow` | MIT | animated numbers |

Free component registries in the shadcn style (Magic UI, motion-primitives,
Cult UI free, Tailark, SmoothUI, Kokonut UI free, Eldora UI) are raw
material: copy the source in, then restyle it to the boards. Never ship a
registry component in its default skin.

## Lane B (Vite)

```bash
cd "<app folder>" && npm install gsap lenis split-type motion
cd "<app folder>" && npm install three @react-three/fiber @react-three/drei   # 3D in React
```

```js
import gsap from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import Lenis from 'lenis';
import SplitType from 'split-type';
gsap.registerPlugin(ScrollTrigger);
```

## Lane A (Nebo compiles)

Import packages bare and pinned; Nebo saves them into the app on write
(versions in `src/vendor.lock.json`), so they load offline:

```ts
import gsap from 'gsap@3.12.5';
import { ScrollTrigger } from 'gsap@3.12.5/ScrollTrigger';
import Lenis from 'lenis@1.1.13';
import SplitType from 'split-type@0.3.4';
import { animate } from 'motion@11.11.17';
import * as THREE from 'three@0.170.0';
import { GLTFLoader } from 'three@0.170.0/examples/jsm/loaders/GLTFLoader.js';
```

In Lane B the same imports drop the `@version` (npm pinned them). GSAP from
cdnjs as a classic script in `index.html`
(`https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js`) is also
proven inside the Nebo app view on desktop and in the mobile app.

## Lenis bridged to GSAP (without the bridge, scrubs stutter)

```js
const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
if (!reduce) {
  const lenis = new Lenis({ autoRaf: false, lerp: 0.1 });
  lenis.on('scroll', ScrollTrigger.update);
  gsap.ticker.add((t) => lenis.raf(t * 1000));
  gsap.ticker.lagSmoothing(0);
}
```

Skip Lenis on a page whose scroll drives a film scrub with the
`film-scrub.md` controller unless you have tested both together; that
controller reads the native scroller directly.

## Reduced motion, once for the whole app

```js
const mm = gsap.matchMedia();
mm.add('(prefers-reduced-motion: no-preference)', () => {
  // every timeline and ScrollTrigger goes here; they are reverted
  // automatically when the person asks for reduced motion
});
```

CSS animations: wrap them in `@media (prefers-reduced-motion: no-preference)`.

## Headline build (split-type + GSAP), screenshot-safe

```js
const split = new SplitType('.hero-title', { types: 'lines,words' });
gsap.from(split.words, { yPercent: 110, duration: 0.9, ease: 'power4.out', stagger: 0.04 });
```

The words start below a line mask (`.line { overflow: hidden }`), never
at `opacity: 0`, and it runs on load, not on a viewport trigger.

## Pointer physics (springs, no state per frame)

```js
import { animate } from 'motion';
const el = document.querySelector('.magnet');
el.addEventListener('pointermove', (e) => {
  const r = el.getBoundingClientRect();
  animate(el, { x: (e.clientX - r.left - r.width / 2) * 0.3,
                y: (e.clientY - r.top - r.height / 2) * 0.3 },
          { type: 'spring', stiffness: 200, damping: 20 });
});
el.addEventListener('pointerleave', () => animate(el, { x: 0, y: 0 }, { type: 'spring' }));
```

On touch screens, replace pointer effects with scroll or tilt equivalents.

## 3D

- Load models with `GLTFLoader` from `ui/assets/*.glb` (Nebo serves them as
  `model/gltf-binary`). Compress large ones (Draco or meshopt): smaller
  loads sooner, and one file may be at most 100 MB.
- One draw call per swarm of the same thing (instancing). Shadows and
  post-processing off unless the brief needs them.
- Cap `devicePixelRatio` at 2. Pause the render loop when the page is
  hidden (`visibilitychange`).
- Environment lighting from a self-hosted HDRI in `ui/assets/`, never a
  preset that fetches from a CDN at runtime.
- Test WebGL2 the way the mobile app loads it (`app_screenshot` at 390x844;
  `app_console` shows context errors).

## Tailwind v4

Lane B only (Lane A writes plain CSS with the brief's tokens as custom
properties). Add the Vite plugin once:

```bash
cd "<app folder>" && npm install --save-dev tailwindcss @tailwindcss/vite
```

In `vite.config.mjs`, `import tailwindcss from '@tailwindcss/vite';` and
`plugins: [react(), tailwindcss()]`. `src/app.css` starts with
`@import "tailwindcss";` and defines the brief's tokens in
`@theme { --color-ground: #E4E8DF; ... }`; `src/main.jsx` imports it with
`import './app.css';`. The build bundles it into `ui/bundle/`. No app-wide
`.btn` classes: each action is styled where it lives.
