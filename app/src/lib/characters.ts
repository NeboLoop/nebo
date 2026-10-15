/**
 * The mountain family: the drawing data every character comes from.
 *
 * The SAME values live in the mobile app's `lib/theme/characters.dart`;
 * `characters.test.ts` and the mobile app's `characters_test.dart` pin them,
 * so the two apps draw one family. The server decides which shape an
 * employee wears (`types::character::shape_of`) and the roster sends it;
 * this file only says how each one looks.
 *
 * Every character sits on a 100 × 100 box, wider than it is tall and
 * lopsided like a real mountain. It is lit from the top left: a radial
 * gradient body (light, base, dark), a shaded right facet, a soft white
 * highlight, rounded corners and a small shadow on itself. Never a glow or
 * a halo behind it. Its face is two eyes (pupil and highlight) that blink,
 * and a small smile. Snow is only ever a small pale-blue patch on one
 * off-centre peak.
 *
 * An employee's colours come from its palette colour: the body runs from
 * BODY_LIGHT (mixed toward white) through the colour to BODY_DARK (mixed
 * toward black); the facet from FACET_FROM to FACET_TO (toward black).
 */

export type CharacterAnim = 'steam' | 'twinkle';

/** What a layer is filled with: the body or facet gradient, the snow patch,
 *  the crater (the body's darkest shade), or a fixed colour. */
export type CharacterFill = 'body' | 'facet' | 'snow' | 'crater' | string;

export interface CharacterLayer {
	/** An SVG path (M, L, Q, C and Z only, so both apps draw it the same). */
	d?: string;
	/** A circle: [cx, cy, r]. */
	circle?: [number, number, number];
	fill: CharacterFill;
	/** Inside the body group (shadowed with it); otherwise drawn above it. */
	body?: boolean;
	opacity?: number;
	anim?: CharacterAnim;
}

/** The soft white highlight: an ellipse [cx, cy, rx, ry], turned `rot` degrees. */
export interface CharacterHighlight {
	at: [number, number, number, number];
	rot: number;
}

/** The face: centred on [cx, cy], at scale `s` (1 = the assistant's). */
export interface CharacterFace {
	at: [number, number];
	s: number;
}

export interface CharacterSpec {
	layers: CharacterLayer[];
	highlight: CharacterHighlight;
	face: CharacterFace;
	/** Fixed colours, for the assistant: [light, base, dark] and [from, to]. */
	body?: [string, string, string];
	facet?: [string, string];
}

/** The face, as both apps draw it from `face.at` and `face.s`. */
export const EYE_DX = 9;
export const EYE_RX = 6;
export const EYE_RY = 6.5;
export const PUPIL_DX = 1.6;
export const PUPIL_DY = 1;
export const PUPIL_R = 3.4;
export const GLINT_DX = 2.8;
export const GLINT_DY = -0.6;
export const GLINT_R = 1.1;
/** The smile: from (±SMILE_HALF, SMILE_Y) through the control point (0, SMILE_DIP). */
export const SMILE_HALF = 6;
export const SMILE_Y = 11;
export const SMILE_DIP = 16.5;
export const SMILE_WIDTH = 2.4;
export const EYE_WHITE = '#FFFFFF';
export const INK = '#0D1A38';

/** The light: the body gradient's centre and reach, in the shape's box. */
export const LIGHT_AT: [number, number] = [0.35, 0.3];
export const LIGHT_REACH = 0.8;
export const BODY_STOPS = [0, 0.55, 1];
/** How far an employee's colour is mixed toward white / black. */
export const BODY_LIGHT = 0.38;
export const BODY_DARK = 0.28;
export const FACET_FROM = 0.12;
export const FACET_TO = 0.45;
export const FACET_OPACITY = 0.85;
export const HIGHLIGHT_ALPHA = 0.55;
/** The shadow on the character itself (never behind it). */
export const SHADOW_BLUR = 1.6;
export const SHADOW_DY = 1.6;
export const SHADOW_ALPHA = 0.35;

export const SNOW_FROM = '#FFFFFF';
export const SNOW_TO = '#B9CEF7';
export const STEAM_NEAR = '#C9D2E6';
export const STEAM_FAR = '#DFE5F1';

/** Brand blue: the assistant's alone. */
export const ASSISTANT_BLUE = '#2F60C9';
export const ASSISTANT_SPARK = '#9DBBF5';
export const ASSISTANT_SHAPE = 'assistant';

const shape = (body: string, facet: string | null, highlight: CharacterHighlight, face: CharacterFace, extra: CharacterLayer[] = []): CharacterSpec => ({
	layers: [{ d: body, fill: 'body', body: true }, ...(facet ? [{ d: facet, fill: 'facet', body: true, opacity: FACET_OPACITY }] : []), ...extra],
	highlight,
	face
});

export const CHARACTERS: Record<string, CharacterSpec> = {
	// Option A, the owner's pick: twin peaks, the taller one off-centre.
	assistant: {
		layers: [
			{ d: 'M3 92 Q2 89 4 86 L28 46 Q30 43 32 46 L43 59 L62 22 Q64 18 66 22 L96 86 Q98 90 94 92 Z', fill: 'body', body: true },
			{ d: 'M64 20 L96 86 Q98 90 94 92 L72 92 L60 52 Z', fill: 'facet', body: true, opacity: FACET_OPACITY },
			{ d: 'M64 20 L70.5 32 L66.5 30 L62.5 35 L59 30 Z', fill: 'snow', body: true },
			{ d: 'M86 2 L92 12 L86 22 L80 12 Z', fill: ASSISTANT_SPARK, anim: 'twinkle' }
		],
		highlight: { at: [34, 60, 11, 6], rot: -35 },
		face: { at: [48, 70], s: 1 },
		body: ['#5E8BF0', ASSISTANT_BLUE, '#1F469E'],
		facet: ['#2A55B5', '#173879']
	},
	ridge: shape(
		'M4 92 Q3 89 5 87 L24 62 Q26 60 28 62 L38 70 L60 42 Q62 39 64 42 L95 87 Q97 90 94 92 Z',
		'M62 40 L95 87 Q97 90 94 92 L74 92 L58 62 Z',
		{ at: [30, 72, 8, 4.5], rot: -35 },
		{ at: [50, 74], s: 0.85 }
	),
	twin: shape(
		'M3 92 Q2 89 4 87 L26 54 Q28 51 30 54 L42 66 L62 36 Q64 33 66 36 L96 87 Q98 90 95 92 Z',
		'M64 34 L96 87 Q98 90 95 92 L74 92 L60 60 Z',
		{ at: [30, 68, 9, 5], rot: -35 },
		{ at: [52, 72], s: 0.85 }
	),
	mesa: shape(
		'M6 92 Q4 89 6 86 L26 48 Q28 45 31 45 L70 42 Q73 42 74 45 L94 86 Q96 89 94 92 Z',
		'M72 42 L94 86 Q96 89 94 92 L76 92 L68 46 Z',
		{ at: [32, 58, 10, 5.5], rot: -30 },
		{ at: [50, 66], s: 0.9 }
	),
	butte: shape(
		'M5 92 Q3 89 5 86 L22 50 Q24 46 28 48 L40 56 L52 46 Q55 44 58 46 L94 86 Q96 89 93 92 Z',
		'M56 45 L94 86 Q96 89 93 92 L74 92 L58 60 Z',
		{ at: [26, 62, 8, 4.5], rot: -40 },
		{ at: [48, 70], s: 0.85 }
	),
	diamond: shape(
		'M4 66 Q2 63 5 61 L40 36 Q43 34 46 35 L94 58 Q97 60 95 63 L58 91 Q55 93 52 91 Z',
		'M46 35 L94 58 Q97 60 95 63 L58 91 L54 60 Z',
		{ at: [30, 54, 9, 4.5], rot: -30 },
		{ at: [46, 64], s: 0.8 }
	),
	dome: shape(
		'M4 92 C6 60 30 40 54 42 C76 44 94 66 96 92 Z',
		'M54 42 C76 44 94 66 96 92 L74 92 C72 70 66 54 54 42 Z',
		{ at: [30, 58, 10, 5.5], rot: -35 },
		{ at: [48, 70], s: 0.9 }
	),
	snowcap: shape(
		'M4 92 Q2 89 4 86 L54 36 Q57 33 60 36 L95 86 Q97 89 94 92 Z',
		'M57 34 L95 86 Q97 89 94 92 L74 92 L54 58 Z',
		{ at: [30, 68, 9, 5], rot: -40 },
		{ at: [46, 72], s: 0.85 },
		[{ d: 'M57 34 L64 43 L60 42 L56 46 L52 42 L50 41 Z', fill: 'snow', body: true }]
	),
	volcano: {
		layers: [
			{ circle: [46, 16, 6], fill: STEAM_NEAR, anim: 'steam' },
			{ circle: [56, 10, 8], fill: STEAM_FAR, anim: 'steam' },
			{ d: 'M6 92 Q4 89 6 86 L34 32 Q35 30 38 30 L62 30 Q65 30 66 32 L94 86 Q96 89 94 92 Z', fill: 'body', body: true },
			{ d: 'M64 30 L94 86 Q96 89 94 92 L76 92 L60 40 Z', fill: 'facet', body: true, opacity: FACET_OPACITY },
			{ d: 'M36 30 L64 30 L58 36 L42 36 Z', fill: 'crater', body: true }
		],
		highlight: { at: [36, 52, 9, 5], rot: -40 },
		face: { at: [50, 62], s: 1 }
	},
	sawtooth: shape(
		'M3 92 Q2 89 4 87 L18 62 L28 72 L42 50 L54 64 L70 40 Q72 37 74 40 L96 87 Q98 90 95 92 Z',
		'M72 38 L96 87 Q98 90 95 92 L78 92 L68 62 Z',
		{ at: [24, 74, 7, 4], rot: -35 },
		{ at: [52, 76], s: 0.8 }
	),
	hook: shape(
		'M5 92 Q3 89 5 86 L32 50 Q34 47 36 46 L42 36 Q45 33 48 35 L60 44 Q62 46 61 48 L94 86 Q96 89 93 92 Z',
		'M48 35 L60 44 Q62 46 61 48 L94 86 Q96 89 93 92 L74 92 L46 56 Z',
		{ at: [30, 70, 8, 4.5], rot: -40 },
		{ at: [46, 72], s: 0.85 }
	),
	flattop: shape(
		'M8 92 Q6 89 8 86 L30 48 Q32 46 35 46 L66 46 Q69 46 71 49 L95 86 Q97 89 94 92 Z',
		'M68 46 L95 86 Q97 89 94 92 L76 92 L64 52 Z',
		{ at: [30, 62, 9, 5], rot: -35 },
		{ at: [48, 70], s: 0.9 },
		[{ d: 'M35 46 L50 46 L47 51 L42 49 L37 52 Z', fill: 'snow', body: true }]
	),
	littlehill: shape(
		'M14 92 C16 70 34 60 52 60 C70 60 84 72 88 92 Z',
		'M52 60 C70 60 84 72 88 92 L70 92 C68 78 62 66 52 60 Z',
		{ at: [32, 74, 6, 3.5], rot: -30 },
		{ at: [50, 79], s: 0.6 }
	)
};

/** The shapes an employee can wear, in the server's order
 *  (`types::character::EMPLOYEE_SHAPES`). */
export const EMPLOYEE_SHAPES = ['ridge', 'twin', 'mesa', 'butte', 'diamond', 'dome', 'snowcap', 'volcano', 'sawtooth', 'hook', 'flattop', 'littlehill'] as const;

/** A stable 31-multiplier hash (the one the colour and shape fallbacks use). */
export function hashOf(key: string): number {
	return [...key].reduce((h, ch) => (Math.imul(h, 31) + ch.charCodeAt(0)) >>> 0, 0);
}

/**
 * The shape to draw: the one the server resolved, or — for an employee the
 * server sent none for (an older bot, an employee borrowed from another bot,
 * a name in a team thread) — a stable one from its id or name. Every
 * employee wears a character; the assistant's is never a fallback.
 */
export function shapeFor(shape: string | null | undefined, key: string): string {
	if (shape && CHARACTERS[shape]) return shape;
	return EMPLOYEE_SHAPES[hashOf(key) % EMPLOYEE_SHAPES.length];
}

/** The spec for `shape`, or null when this app does not know it. */
export function characterOf(shape: string | null | undefined): CharacterSpec | null {
	return (shape && CHARACTERS[shape]) || null;
}

let drawn = 0;

/**
 * A character as SVG markup: the ONE drawing of the spec on the web.
 * AgentAvatar renders it, and so do the mention chips built as HTML strings.
 * `colorClass` names the employee's palette colour (app.css sets `--c` from
 * it, and the gradient stops mix that colour lighter and darker); `cls`
 * sizes the SVG. Each drawing has its own gradient and filter ids.
 */
export function characterSvg(shape: string, key: string, colorClass: string, cls: string): string {
	const spec = CHARACTERS[shapeFor(shape, key)];
	const id = `ch${++drawn}`;
	const beat = `character-beat-${hashOf(key) % 5}`;
	const stop = (offset: number, fixed: string | undefined, cssClass: string) =>
		fixed ? `<stop offset="${offset}" stop-color="${fixed}"/>` : `<stop offset="${offset}" class="${cssClass}"/>`;
	const [cx, cy] = LIGHT_AT;
	const defs =
		`<defs>` +
		`<radialGradient id="${id}b" cx="${cx * 100}%" cy="${cy * 100}%" r="${LIGHT_REACH * 100}%">` +
		stop(BODY_STOPS[0], spec.body?.[0], 'character-light') +
		stop(BODY_STOPS[1], spec.body?.[1], 'character-base') +
		stop(BODY_STOPS[2], spec.body?.[2], 'character-dark') +
		`</radialGradient>` +
		`<linearGradient id="${id}f" x1="0" y1="0" x2="1" y2="1">` +
		stop(0, spec.facet?.[0], 'character-facet-from') +
		stop(1, spec.facet?.[1], 'character-facet-to') +
		`</linearGradient>` +
		`<radialGradient id="${id}s" cx="40%" cy="30%" r="80%"><stop offset="0" stop-color="${SNOW_FROM}"/><stop offset="1" stop-color="${SNOW_TO}"/></radialGradient>` +
		`<radialGradient id="${id}h"><stop offset="0" stop-color="#FFFFFF" stop-opacity="${HIGHLIGHT_ALPHA}"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/></radialGradient>` +
		`<filter id="${id}d" x="-10%" y="-10%" width="120%" height="130%"><feGaussianBlur in="SourceAlpha" stdDeviation="${SHADOW_BLUR}"/><feOffset dx="0" dy="${SHADOW_DY}"/><feComponentTransfer><feFuncA type="linear" slope="${SHADOW_ALPHA}"/></feComponentTransfer><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter>` +
		`</defs>`;
	const paint = (l: CharacterLayer) => {
		const fill =
			l.fill === 'body' ? `url(#${id}b)` : l.fill === 'facet' ? `url(#${id}f)` : l.fill === 'snow' ? `url(#${id}s)` : l.fill === 'crater' ? null : l.fill;
		const classes = [l.fill === 'crater' ? 'character-crater' : '', l.anim ? `character-${l.anim}` : ''].filter(Boolean).join(' ');
		const attrs = [fill ? `fill="${fill}"` : '', l.opacity !== undefined ? `opacity="${l.opacity}"` : '', classes ? `class="${classes}"` : '']
			.filter(Boolean)
			.join(' ');
		return l.circle ? `<circle cx="${l.circle[0]}" cy="${l.circle[1]}" r="${l.circle[2]}" ${attrs}/>` : `<path d="${l.d}" ${attrs}/>`;
	};
	const [hx, hy, hrx, hry] = spec.highlight.at;
	const behind = spec.layers.filter((l) => !l.body && l.anim === 'steam').map(paint).join('');
	const body =
		`<g filter="url(#${id}d)">` +
		spec.layers.filter((l) => l.body).map(paint).join('') +
		`<ellipse cx="${hx}" cy="${hy}" rx="${hrx}" ry="${hry}" fill="url(#${id}h)" transform="rotate(${spec.highlight.rot} ${hx} ${hy})"/>` +
		`</g>`;
	const above = spec.layers.filter((l) => !l.body && l.anim !== 'steam').map(paint).join('');
	const [fx, fy] = spec.face.at;
	const s = spec.face.s;
	const eye = (x: number) =>
		`<g class="character-eye ${beat}"><ellipse cx="${x}" cy="${fy}" rx="${EYE_RX * s}" ry="${EYE_RY * s}" fill="${EYE_WHITE}"/>` +
		`<circle cx="${x + PUPIL_DX * s}" cy="${fy + PUPIL_DY * s}" r="${PUPIL_R * s}" fill="${INK}"/>` +
		`<circle cx="${x + GLINT_DX * s}" cy="${fy + GLINT_DY * s}" r="${GLINT_R * s}" fill="${EYE_WHITE}"/></g>`;
	const smile = `<path d="M${fx - SMILE_HALF * s} ${fy + SMILE_Y * s} Q${fx} ${fy + SMILE_DIP * s} ${fx + SMILE_HALF * s} ${fy + SMILE_Y * s}" stroke="${INK}" stroke-width="${SMILE_WIDTH * s}" fill="none" stroke-linecap="round"/>`;
	const face = eye(fx - EYE_DX * s) + eye(fx + EYE_DX * s) + smile;
	return `<svg class="character ${colorClass} shrink-0 select-none ${cls}" viewBox="0 0 100 100" aria-hidden="true">${defs}${behind}${body}${above}${face}</svg>`;
}
