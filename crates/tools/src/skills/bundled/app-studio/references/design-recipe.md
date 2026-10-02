# Design Recipe (the craft floor, read on every build)

Every rule here exists because default model output breaks it. Follow it as
written. The boards decide composition; this file decides the floor no
board may go under.

## 1. Typography

- Display scale: about `clamp(2.25rem, 6vw, 4.5rem)`, tight tracking,
  `line-height: 1`. Go bigger only when the headline is 3 to 5 words. A
  four-line hero headline is a size error, not a copy error.
- Body: 16 to 18px, `line-height` about 1.6, measure at most 65ch.
- Pairings to start from (then pick by the brief, never by habit): Geist +
  Geist Mono; Satoshi + JetBrains Mono; Cabinet Grotesk + Inter Tight;
  Outfit + IBM Plex Mono; Clash Display + Manrope; Space Grotesk + Space
  Mono; General Sans + Fragment Mono. Load from Google Fonts or Fontshare,
  or self-host the `.woff2` in `ui/fonts/`.
- **Inter as display is banned.** Serif is never the default for
  "premium"; use one only with a written reason in the brief. Fraunces and
  Instrument Serif are banned as defaults.
- Emphasis inside a headline: italic or bold of the same family. Never drop
  a serif word into a sans headline "for interest".
- Italic display words with descenders need `line-height` of at least 1.1
  and a little bottom padding, or the descenders clip.

## 2. Color

- Exactly one accent, locked app-wide. A rose-accented app does not get a
  teal badge in the footer.
- Neutral ground: pick warm or cool, never both. No pure `#000000`; use an
  off-black. No neon glows.
- One theme per app. Tint shifts within the family are fine.
- **Banned families** (the gate greps for these hexes; overridable only by
  the owner's own brand, recorded as `brand_override` in the brief):
  1. Near-black plus orange, amber or ember: `#ff5c1a #ff6b35 #e8590c
     #f97316 #ea580c #d9480f #ff4b1f #f59e0b` on `#0a0a0a`-like grounds.
  2. Near-black plus neon cyan, blue or green: `#00e5ff #22d3ee #00ff88
     #4ade80 #3b82f6 #00ffc2`.
  3. Purple or violet glow: `#8b5cf6 #a855f7 #7c3aed #6d28d9`.
  4. Beige or cream plus brass, clay or oxblood. Grounds `#f5f1ea #f7f5f1
     #fbf8f1 #efeae0 #ece6db #faf7f1 #e8dfcb`; accents `#b08947 #b6553a
     #9a2436 #9c6e2a #bc7c3a #7d5621`; inks `#1a1714 #1a1814 #1b1814`.
  5. The palette family of any other app on this bot.
- Where to reach instead: derive from the owner's material world first.
  If that lands in a banned family, escape with a bold solid field (bottle
  green, royal blue, vermilion, emerald), a chromatic light (limestone,
  celadon, warm grey) with one unexpected accent (chartreuse, ultramarine,
  vermilion-pink), or a duotone photographic palette. These are examples,
  not a menu: do not take the first one every time.

## 3. Hero (or first screen)

- Fits the first viewport on a phone and on a desktop: headline at most 2
  lines on desktop, sub-line at most 20 words, the main action visible
  without scrolling.
- At most 4 text elements: an optional eyebrow or brand strip, the
  headline, one sub-line, one action row (one primary, at most one
  secondary). Never in the hero: feature bullets, trust strips, avatar
  rows, version labels, pricing teasers.
- A real visual: a generated image, film or 3D subject, or (in
  no-generation mode) real canvas or shader art. Text over a gradient blob
  is a placeholder. A fake product UI built from divs is the most common
  tell; use a generated image of the product, the real component, or
  nothing.
- Avoid the centered stack unless the brief is a manifesto. Avoid opening
  on left-text and right-image; it is the most overused layout.

## 4. Layout

- Each layout family (card grid, split text and image, full-width quote,
  bento, rail) at most once per app; 6 or more screens need at least 4
  families. At most 2 zigzag splits in a row.
- No row of three equal feature cards. Use an asymmetric grid, a two-up, a
  rail or a list.
- Bento: exactly as many cells as items, and 2 or 3 cells carry real visual
  variation.
- Eyebrows (small uppercase kickers above a headline): at most one per
  three screens. Usually drop them.
- Cards only where elevation means hierarchy; otherwise rules, dividers or
  space. One corner language for the whole app: all sharp, all soft (12 to
  16px), or all pill.
- Use `dvh` units (`min-height: 100dvh`), never `100vh` or `h-screen`.
- Declare how every multi-column screen collapses on a phone.
- Nothing may be wider than the screen. Long unbroken strings get
  `overflow-wrap: anywhere`. The page pans vertically only:
  `html, body { touch-action: pan-y; overscroll-behavior-x: none; }`
  (games that take every gesture use `touch-action: none` on the play area
  instead).

## 5. Copy

- Headline at most 8 words; a paragraph at most 25 words; per screen one
  visual or one action, not a pile.
- **No em-dashes or en-dash separators anywhere visible.** Use a period,
  comma, colon, parentheses or a hyphen.
- One label per intent app-wide. "Get in touch", "Contact us" and "Let's
  talk" on one app is a failure; pick one.
- Primary action labels: 3 words or fewer, one line.
- Banned: Elevate, Seamless, Unleash, Next-Gen, Revolutionize; "Acme",
  "Nexus", "Jane Doe"; invented performance stats ("92% faster", "10k+
  teams"). Invented product facts in a fictional catalog (prices, sizes)
  are fine if plausible and consistent.
- Also banned: section numbering ("001 / Capabilities"), "Scroll to
  explore", decorative status dots, pills laid over photos, version
  footers.
- Reread every visible string before the gate. Rewrite anything vague,
  broken, or "trying to sound thoughtful". Plain beats clever.

## 6. Motion

- One signature effect (the brief's), plus reveals that mean something:
  hierarchy, story, feedback or state. "It looked cool" means cut it.
- Springs over linear easing for UI. Pointer and drag physics write to
  transforms directly, never through a state update per frame.
- Animate `transform` and `opacity` on the hot path; never layout
  properties.
- **Screenshot-safe:** nothing waits at `opacity: 0` for a scroll or
  viewport trigger. Text builds fire on load; scroll-linked effects move,
  scale or clip, they do not fade from nothing. A screenshot at any scroll
  position shows every screen's content.
- **`prefers-reduced-motion`** on every animated element: the composed end
  state, static.
- No custom cursor except the spectacle tier on desktop.
- If motion cannot be finished properly, ship a clean still page instead
  of half-wired triggers.

## 7. States and forms

- Every async view has a loading state shaped like the final layout (no
  generic spinner), a composed empty state, and an inline error state.
- Pressed feedback on every tappable thing (`scale(0.98)` or a 1px drop).
- Every button label passes WCAG AA against its own background; ghost
  buttons over images get a scrim or stroke.
- Forms: label above the field, error below it, never a placeholder as the
  label.
- Touch targets at least 44 by 44px. No hover-only interactions.

## 8. Images and icons

- Order of preference: the owner's own assets, then generated ones, then
  (no-generation mode) hand-made SVG, canvas or shader art. Stock photos
  and placeholder services are never the final state.
- Even a minimal app needs 2 or 3 real images.
- Icons: the generated set first, one stroke style, in the palette. A
  permissive icon library (Phosphor, Tabler, Lucide) only for dense
  functional UI. Never mix both in one zone.
- Logos of real companies: real SVG marks only. An invented brand gets a
  generated or drawn monogram, not a styled `<span>`.
