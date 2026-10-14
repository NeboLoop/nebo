// The mountain family is drawn from one spec in each app: this file and the
// mobile app's lib/theme/characters.dart carry the same values, and both
// pin them, so a change to one that misses the other fails a test.
import { describe, it, expect } from 'vitest';
import { render } from 'svelte/server';
import AgentAvatar from '$lib/components/AgentAvatar.svelte';
import { CHARACTERS, EMPLOYEE_SHAPES, ASSISTANT_SHAPE, ASSISTANT_BLUE, characterOf, shapeFor } from './characters';

describe('the mountain family', () => {
	it('has a character for every shape the server can send', () => {
		// types::character::EMPLOYEE_SHAPES, plus the assistant's own.
		expect([...EMPLOYEE_SHAPES]).toEqual(['ridge', 'twin', 'mesa', 'butte', 'diamond', 'dome', 'snowcap', 'volcano', 'spire', 'sawtooth', 'hook', 'flattop', 'littlehill']);
		expect(Object.keys(CHARACTERS).sort()).toEqual([ASSISTANT_SHAPE, ...EMPLOYEE_SHAPES].sort());
		for (const spec of Object.values(CHARACTERS)) {
			expect(spec.eyes).toHaveLength(2);
			expect(spec.layers.length).toBeGreaterThan(0);
			for (const l of spec.layers) expect(!!l.d !== !!l.circle).toBe(true);
		}
	});

	it('pins the path data the mobile app shares', () => {
		expect(CHARACTERS.assistant.layers[0]).toEqual({ d: 'M50 6 L95 92 L5 92 Z', fill: '#2F60C9' });
		expect(CHARACTERS.assistant.layers[3].anim).toBe('twinkle');
		expect(CHARACTERS.ridge.layers[0].d).toBe('M4 92 L24 52 L38 66 L60 22 L96 92 Z');
		expect(CHARACTERS.dome.layers[0].d).toBe('M8 92 C8 28 92 28 92 92 Z');
		expect(CHARACTERS.volcano.layers.filter((l) => l.anim === 'steam')).toHaveLength(2);
		expect(CHARACTERS.spire.eyeR).toBe(4.5);
		expect(CHARACTERS.littlehill.eyes).toEqual([[43, 76], [57, 76]]);
	});

	it('keeps brand blue for the assistant alone', () => {
		for (const [shape, spec] of Object.entries(CHARACTERS)) {
			if (shape === ASSISTANT_SHAPE) continue;
			for (const l of spec.layers) expect(l.fill).not.toBe(ASSISTANT_BLUE);
		}
	});
});

describe('AgentAvatar', () => {
	it('draws the character the server resolved, in the employee colour', () => {
		const { body } = render(AgentAvatar, { props: { name: 'Store Manager', color: 'violet', shape: 'ridge' } });
		expect(body).toContain('M4 92 L24 52 L38 66 L60 22 L96 92 Z');
		expect(body).toContain('fill-[var(--agent-violet-ink)]');
		expect(body).toContain('character-eye');
	});

	it('never fills an employee in the brand colour', () => {
		const { body } = render(AgentAvatar, { props: { name: 'Piper', id: 'e-9', color: 'teal', shape: 'dome' } });
		expect(body).not.toContain('fill-primary');
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
		// types::character::shape_for_id pins "a" → snowcap and "zz" → diamond.
		expect(shapeFor(null, 'a')).toBe('snowcap');
		expect(shapeFor(null, 'zz')).toBe('diamond');
		expect(shapeFor('nonsense', 'a')).toBe('snowcap');
		for (const key of ['x', 'assistant-ish', 'Nebo', 'Dana Cole']) expect(shapeFor(undefined, key)).not.toBe(ASSISTANT_SHAPE);
	});
});
