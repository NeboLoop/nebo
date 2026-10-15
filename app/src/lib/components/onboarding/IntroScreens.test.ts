// The three intro screens before the guided first-run chat: drawn in code
// with the mountain family, one button per screen (Next, Next, Get started),
// Skip on top, and still under prefers-reduced-motion. Get started and Skip
// both leave through startFirstRun, which starts the server's chat.
import { describe, it, expect, vi, beforeAll } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import { readFileSync } from 'node:fs';
import en from '$lib/i18n/locales/en.json';

const api = vi.hoisted(() => ({ startOnboarding: vi.fn(async () => ({ status: { step: 'name' }, threadId: 't9' })) }));
vi.mock('$lib/api/nebo', () => api);

import IntroScreens from './IntroScreens.svelte';
import { startFirstRun } from '$lib/stores/onboarding';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const text = (html: string) => html.replace(/<!--[^>]*-->/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
const html = () => render(IntroScreens, { props: { assistantName: 'Aspen', onstarted: () => {} } }).body;

describe('IntroScreens', () => {
	it('has the three screens, in order, with one button each', () => {
		const body = html();
		expect(body.match(/class="intro-screen[ "]/g)).toHaveLength(3);
		const words = text(body);
		const order = ['Meet your assistant', 'Works on its own computer, even while you sleep', 'Hand off tasks, on demand or automated'].map((h) => words.indexOf(h));
		expect(order.every((i) => i >= 0)).toBe(true);
		expect([...order].sort((a, b) => a - b)).toEqual(order);
		const buttons = [...body.matchAll(/<button[^>]*class="intro-next"[^>]*>([^<]*)<\/button>/g)].map((m) => m[1]);
		expect(buttons).toEqual(['Next', 'Next', 'Get started']);
		expect(words).toContain('Skip');
		expect(body.match(/<div class="intro-dots"/g)).toHaveLength(3);
	});

	it('shows the first screen and keeps the others inert', () => {
		const body = html();
		expect(body.match(/intro-screen is-active/g)).toHaveLength(1);
		expect(body.match(/<section[^>]*inert/g)).toHaveLength(2);
	});

	it('draws the assistant and the team from the character spec', () => {
		const body = html();
		expect(body).toContain('L62 22 Q64 18 66 22'); // the assistant, twin peaks
		expect(body).toContain('L60 42 Q62 39 64 42'); // Store Manager, ridge
		expect(body).toContain('character-peach'); // Content, volcano
		const words = text(body);
		for (const s of ['Aspen · Assistant', 'Store Manager', 'Weekly standup', 'Reordered 40 units from Lumen', 'Sent 3 invoice reminders']) {
			expect(words).toContain(s);
		}
	});

	it('leaves through startFirstRun from Skip and from Get started', async () => {
		const src = readFileSync('src/lib/components/onboarding/IntroScreens.svelte', 'utf8');
		expect(src).toMatch(/class="intro-skip" onclick=\{begin\}/);
		expect(src).toMatch(/onclick=\{begin\}[^>]*>\{\$t\('firstRun\.getStarted'\)\}/);
		expect(src).toMatch(/onstarted\(await startFirstRun\(\)\)/);
		expect(await startFirstRun()).toBe('t9');
		expect(api.startOnboarding).toHaveBeenCalled();
	});

	it('puts no glow or halo behind a character', () => {
		expect(html()).not.toContain('intro-sun');
		expect(readFileSync('src/app.css', 'utf8')).not.toContain('.intro-sun');
	});

	it('is still under prefers-reduced-motion', () => {
		expect(html()).toContain('class="intro-screens"');
		const css = readFileSync('src/app.css', 'utf8');
		const start = css.indexOf('@media (prefers-reduced-motion: no-preference)');
		expect(start).toBeGreaterThan(-1);
		// Every intro and burst animation sits inside the no-preference block.
		const outside = css.slice(0, start) + css.slice(css.indexOf('@keyframes intro-pop'));
		expect(outside).not.toMatch(/\.intro-[^{]*\{[^}]*animation:/);
		expect(outside).not.toMatch(/\.first-run-burst[^{]*\{[^}]*animation:/);
		expect(css.slice(start, css.indexOf('@keyframes intro-pop'))).toContain('.is-active .intro-hero { animation');
	});
});
