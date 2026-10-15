// The mountain family is drawn from one spec in each app: this file and the
// mobile app's lib/theme/characters.dart carry the same values, and both
// pin them, so a change to one that misses the other fails a test.
import { describe, it, expect } from 'vitest';
import { render } from 'svelte/server';
import AgentAvatar from '$lib/components/AgentAvatar.svelte';
import { CHARACTERS, EMPLOYEE_SHAPES, ASSISTANT_SHAPE, ASSISTANT_BLUE, characterOf, characterSvg, shapeFor } from './characters';

/** Every coordinate a path names, as [x, y] pairs. */
const points = (d: string) => {
	const n = (d.match(/-?\d+(\.\d+)?/g) ?? []).map(Number);
	return n.flatMap((_, i) => (i % 2 ? [] : [[n[i], n[i + 1]] as [number, number]]));
};

describe('the mountain family', () => {
	it('has a character for every shape the server can send, and no spire', () => {
		// types::character::EMPLOYEE_SHAPES, plus the assistant's own.
		expect([...EMPLOYEE_SHAPES]).toEqual(['ridge', 'twin', 'mesa', 'butte', 'diamond', 'dome', 'snowcap', 'volcano', 'sawtooth', 'hook', 'flattop', 'littlehill']);
		expect(Object.keys(CHARACTERS).sort()).toEqual([ASSISTANT_SHAPE, ...EMPLOYEE_SHAPES].sort());
		for (const spec of Object.values(CHARACTERS)) {
			expect(spec.layers.filter((l) => l.fill === 'body')).toHaveLength(1);
			for (const l of spec.layers) expect(!!l.d !== !!l.circle).toBe(true);
		}
	});

	it('draws every employee wider than tall and lopsided', () => {
		for (const name of EMPLOYEE_SHAPES) {
			const body = CHARACTERS[name].layers.find((l) => l.fill === 'body')!.d!;
			const p = points(body);
			const xs = p.map(([x]) => x);
			const ys = p.map(([, y]) => y);
			const w = Math.max(...xs) - Math.min(...xs);
			const h = Math.max(...ys) - Math.min(...ys);
			expect(w / h, name).toBeGreaterThanOrEqual(1.4);
			// Its top is off the middle: never a symmetric cone.
			const top = p.reduce((a, b) => (b[1] < a[1] ? b : a));
			const mid = (Math.max(...xs) + Math.min(...xs)) / 2;
			expect(Math.abs(top[0] - mid), name).toBeGreaterThan(2);
		}
	});

	it('keeps snow a small pale-blue patch and brand blue for the assistant alone', () => {
		for (const [shape, spec] of Object.entries(CHARACTERS)) {
			for (const l of spec.layers) expect(l.fill.toUpperCase()).not.toBe('#FFFFFF');
			if (shape === ASSISTANT_SHAPE) continue;
			expect(spec.body).toBeUndefined();
			for (const l of spec.layers) expect(l.fill).not.toBe(ASSISTANT_BLUE);
		}
	});

	it('pins the path data the mobile app shares', () => {
		expect(CHARACTERS.assistant.layers[0].d).toBe('M3 92 Q2 89 4 86 L28 46 Q30 43 32 46 L43 59 L62 22 Q64 18 66 22 L96 86 Q98 90 94 92 Z');
		expect(CHARACTERS.assistant.layers[3]).toEqual({ d: 'M86 2 L92 12 L86 22 L80 12 Z', fill: '#9DBBF5', anim: 'twinkle' });
		expect(CHARACTERS.assistant.body).toEqual(['#5E8BF0', '#2F60C9', '#1F469E']);
		expect(CHARACTERS.assistant.face).toEqual({ at: [48, 70], s: 1 });
		expect(CHARACTERS.ridge.layers[0].d).toBe('M4 92 Q3 89 5 87 L24 62 Q26 60 28 62 L38 70 L60 42 Q62 39 64 42 L95 87 Q97 90 94 92 Z');
		expect(CHARACTERS.dome.layers[0].d).toBe('M4 92 C6 60 30 40 54 42 C76 44 94 66 96 92 Z');
		expect(CHARACTERS.volcano.layers.filter((l) => l.anim === 'steam')).toHaveLength(2);
		expect(CHARACTERS.littlehill.face).toEqual({ at: [50, 79], s: 0.6 });
		expect(CHARACTERS.snowcap.highlight).toEqual({ at: [30, 68, 9, 5], rot: -40 });
	});
});

describe('AgentAvatar', () => {
	it('draws the character the server resolved, shaded from the employee colour', () => {
		const { body } = render(AgentAvatar, { props: { name: 'Store Manager', color: 'violet', shape: 'ridge' } });
		expect(body).toContain('L60 42 Q62 39 64 42');
		expect(body).toContain('character-violet');
		expect(body).toContain('character-light');
		expect(body).toContain('character-eye');
		expect(body).toContain('stroke-linecap="round"'); // the smile
	});

	it('gives each drawing its own gradient ids, and no glow behind it', () => {
		const a = characterSvg('ridge', 'a', 'character-violet', '');
		const b = characterSvg('ridge', 'a', 'character-violet', '');
		const id = (s: string) => s.match(/radialGradient id="([^"]+)"/)![1];
		expect(id(a)).not.toBe(id(b));
		expect(a).not.toMatch(/glow|halo/);
	});

	it('never shades an employee from the brand colour', () => {
		const { body } = render(AgentAvatar, { props: { name: 'Piper', id: 'e-9', color: 'teal', shape: 'dome' } });
		expect(body).not.toContain('character-teal');
	});

	it('draws the assistant in brand blue with its spark', () => {
		const { body } = render(AgentAvatar, { props: { name: 'Nebo', shape: 'assistant' } });
		expect(body).toContain('#2F60C9');
		expect(body).toContain('character-twinkle');
	});

	it('gives every employee a character, even with no shape from the server', () => {
		const { body } = render(AgentAvatar, { props: { name: 'Dana Cole', id: 'e-1', color: 'violet' } });
		expect(body).toContain('character-eye');
		expect(body).not.toContain('>DC<');
		expect(characterOf('nonsense')).toBeNull();
	});

	it('falls back on the same id hash as the server, never to the assistant', () => {
		// types::character::shape_for_id pins "a" → twin and "zz" → diamond.
		expect(shapeFor(null, 'a')).toBe('twin');
		expect(shapeFor(null, 'zz')).toBe('diamond');
		expect(shapeFor('nonsense', 'a')).toBe('twin');
		expect(shapeFor('spire', 'a')).toBe('twin');
		for (const key of ['x', 'assistant-ish', 'Nebo', 'Dana Cole']) expect(shapeFor(undefined, key)).not.toBe(ASSISTANT_SHAPE);
	});
});
