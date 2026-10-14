/**
 * The mountain family: the path data every character is drawn from.
 *
 * The SAME values live in the mobile app's `lib/theme/characters.dart`;
 * `characters.test.ts` and the mobile app's `characters_test.dart` pin them,
 * so the two apps draw one family. The server decides which shape an
 * employee wears (`types::character::shape_of`) and the roster sends it;
 * this file only says how each one looks.
 *
 * Every character sits on a 100 × 100 box. A layer's fill is `body` (the
 * employee's colour) or a fixed colour. Eyes are a white disc with a navy
 * pupil, and they blink.
 */

export type CharacterAnim = 'steam' | 'twinkle';

export interface CharacterLayer {
	/** An SVG path (M, L, C and Z only, so both apps draw it the same). */
	d?: string;
	/** A circle: [cx, cy, r]. */
	circle?: [number, number, number];
	/** 'body' is the employee's colour. */
	fill: string;
	anim?: CharacterAnim;
}

export interface CharacterSpec {
	layers: CharacterLayer[];
	eyes: [number, number][];
	eyeR: number;
}

export const EYE_WHITE = '#FFFFFF';
export const PUPIL = '#0D1A38';
/** The pupil's size and offset, as fractions of the eye's radius. */
export const PUPIL_R = 0.5;
export const PUPIL_DX = 0.2;
export const PUPIL_DY = 0.1;

/** Brand blue: the assistant's alone. */
export const ASSISTANT_BLUE = '#2F60C9';
export const ASSISTANT_SHADE = '#244FA8';
export const ASSISTANT_SPARK = '#8FB0F2';
export const SNOW = '#FFFFFF';
export const CRATER = '#C4513A';
export const STEAM_NEAR = '#C9D2E6';
export const STEAM_FAR = '#DFE5F1';

export const ASSISTANT_SHAPE = 'assistant';

export const CHARACTERS: Record<string, CharacterSpec> = {
	assistant: {
		layers: [
			{ d: 'M50 6 L95 92 L5 92 Z', fill: ASSISTANT_BLUE },
			{ d: 'M50 6 L95 92 L60 92 Z', fill: ASSISTANT_SHADE },
			{ d: 'M50 6 L62 29 L55 25 L50 32 L45 25 L38 29 Z', fill: SNOW },
			{ d: 'M83 2 L90 13 L83 24 L76 13 Z', fill: ASSISTANT_SPARK, anim: 'twinkle' }
		],
		eyes: [[39, 66], [54, 66]],
		eyeR: 6
	},
	ridge: { layers: [{ d: 'M4 92 L24 52 L38 66 L60 22 L96 92 Z', fill: 'body' }], eyes: [[56, 60], [68, 60]], eyeR: 5.5 },
	twin: { layers: [{ d: 'M4 92 L30 40 L44 62 L66 14 L96 92 Z', fill: 'body' }], eyes: [[60, 58], [72, 58]], eyeR: 5.5 },
	mesa: { layers: [{ d: 'M12 92 L30 36 L70 36 L88 92 Z', fill: 'body' }], eyes: [[41, 60], [59, 60]], eyeR: 5.5 },
	butte: { layers: [{ d: 'M8 92 L26 30 L46 46 L58 30 L92 92 Z', fill: 'body' }], eyes: [[30, 62], [42, 62]], eyeR: 5.5 },
	diamond: { layers: [{ d: 'M50 6 L90 50 L50 94 L10 50 Z', fill: 'body' }], eyes: [[42, 48], [58, 48]], eyeR: 5.5 },
	dome: { layers: [{ d: 'M8 92 C8 28 92 28 92 92 Z', fill: 'body' }], eyes: [[42, 64], [58, 64]], eyeR: 5.5 },
	snowcap: {
		layers: [
			{ d: 'M50 8 L92 92 L8 92 Z', fill: 'body' },
			{ d: 'M50 8 L65 38 L57 33 L50 40 L43 33 L35 38 Z', fill: SNOW }
		],
		eyes: [[42, 64], [58, 64]],
		eyeR: 5.5
	},
	volcano: {
		layers: [
			{ circle: [46, 16, 6], fill: STEAM_NEAR, anim: 'steam' },
			{ circle: [56, 10, 8], fill: STEAM_FAR, anim: 'steam' },
			{ d: 'M8 92 L36 30 L64 30 L92 92 Z', fill: 'body' },
			{ d: 'M36 30 L64 30 L58 36 L42 36 Z', fill: CRATER }
		],
		eyes: [[42, 60], [58, 60]],
		eyeR: 5.5
	},
	spire: { layers: [{ d: 'M50 4 L64 92 L36 92 Z', fill: 'body' }], eyes: [[45, 62], [55, 62]], eyeR: 4.5 },
	sawtooth: { layers: [{ d: 'M4 92 L18 50 L30 70 L44 34 L56 62 L70 26 L96 92 Z', fill: 'body' }], eyes: [[64, 62], [76, 62]], eyeR: 5.5 },
	hook: { layers: [{ d: 'M14 92 L44 40 L48 14 L64 30 L60 44 L90 92 Z', fill: 'body' }], eyes: [[48, 64], [60, 64]], eyeR: 5.5 },
	flattop: {
		layers: [
			{ d: 'M12 92 L32 38 L68 38 L88 92 Z', fill: 'body' },
			{ d: 'M32 38 L68 38 L64 48 L56 44 L50 50 L44 44 L36 48 Z', fill: SNOW }
		],
		eyes: [[41, 64], [59, 64]],
		eyeR: 5.5
	},
	littlehill: { layers: [{ d: 'M22 92 C22 56 78 56 78 92 Z', fill: 'body' }], eyes: [[43, 76], [57, 76]], eyeR: 4.5 }
};

/** The shapes an employee can wear, in the server's order
 *  (`types::character::EMPLOYEE_SHAPES`). */
export const EMPLOYEE_SHAPES = ['ridge', 'twin', 'mesa', 'butte', 'diamond', 'dome', 'snowcap', 'volcano', 'spire', 'sawtooth', 'hook', 'flattop', 'littlehill'] as const;

/** The spec for `shape`, or null when this app does not know it. */
export function characterOf(shape: string | null | undefined): CharacterSpec | null {
	return (shape && CHARACTERS[shape]) || null;
}
