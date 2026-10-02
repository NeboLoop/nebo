import { describe, it, expect, vi, beforeAll } from 'vitest';
import { render } from 'svelte/server';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';

// The page reads and writes the bot's setting; nothing here may reach a server.
vi.mock('$lib/api/nebo', () => ({
	getSettings: vi.fn(async () => ({ settings: { developerMode: true, appDeveloperMode: false } })),
	updateSettings: vi.fn(async () => ({ settings: { developerMode: true, appDeveloperMode: false } }))
}));

import Page from './+page.svelte';
import { devMode } from '$lib/stores/devmode';

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('Settings → Developer', () => {
	it('shows App Developer mode only while Developer mode is on', () => {
		devMode.set(false);
		const off = render(Page).body;
		expect(off).toContain(en.settingsDeveloper.devMode);
		expect(off).not.toContain(en.settingsDeveloper.appDevMode);

		devMode.set(true);
		const on = render(Page).body;
		expect(on).toContain(en.settingsDeveloper.appDevMode);
	});

	it('offers no sideloading: there is no backend for it', () => {
		devMode.set(true);
		const body = render(Page).body;
		for (const fake of ['My Custom Tool', 'Test Plugin', en.settingsDeveloper.relaunch, en.settingsDeveloper.unload, en.settingsDeveloper.sideloadApp]) {
			expect(body).not.toContain(fake);
		}
	});
});
