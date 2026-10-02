import { derived, writable } from 'svelte/store';
import { storage } from '$lib/storage';
import { getSettings, updateSettings } from '$lib/api/nebo';

/**
 * Developer mode and App Developer mode: the bot's own settings
 * (`developerMode`, `appDeveloperMode` on GET/PUT /api/v1/agent/settings),
 * so the desktop, the web app and the phone all see the same switch. The ONE
 * source of truth; nothing here is kept per browser.
 *
 * Developer mode gates the advanced surfaces (employee Webhooks, API, the
 * memory inspector, writing setup questions, the workflow canvas, raw run
 * input, ...). App Developer mode gates the app builder tools (Publish, the
 * console) and only counts while Developer mode is on.
 *
 * Both read false until the bot answers, so a normal owner never sees
 * developer controls flash in. The first subscriber reads the bot's setting,
 * so every window (the workspace, an app window, Settings) is right without
 * having to remember to load it.
 */
export const devMode = writable(false, () => {
	void loadDevMode();
});
const appDevSetting = writable(false);

/** App Developer mode as it takes effect: on only while Developer mode is on. */
export const appDeveloperMode = derived([devMode, appDevSetting], ([dev, app]) => dev && app);
/** The saved App Developer switch itself, for the toggle on Settings → Developer. */
export const appDeveloperSetting = { subscribe: appDevSetting.subscribe };

// Before the switch lived on the bot it lived in this browser's storage. An
// owner who had turned it on keeps it: the first read moves it to the bot.
const LEGACY_KEY = 'nebo-devmode';

function apply(s: { developerMode?: boolean; appDeveloperMode?: boolean } | undefined) {
	devMode.set(!!s?.developerMode);
	appDevSetting.set(!!s?.appDeveloperMode);
}

export async function loadDevMode(): Promise<void> {
	try {
		const res = await getSettings();
		let s = res?.settings;
		if (!s?.developerMode && storage.get(LEGACY_KEY) === 'true') {
			s = (await updateSettings({ developerMode: true }))?.settings ?? s;
		}
		storage.remove(LEGACY_KEY);
		apply(s);
	} catch {
		// Keep what was known: a failed read never shows developer controls.
	}
}

/** Turn Developer mode on or off. Off also turns App Developer mode off. */
export async function setDevMode(on: boolean): Promise<void> {
	devMode.set(on);
	try {
		const res = await updateSettings(on ? { developerMode: true } : { developerMode: false, appDeveloperMode: false });
		apply(res?.settings);
	} catch {
		await loadDevMode();
	}
}

export async function setAppDeveloperMode(on: boolean): Promise<void> {
	appDevSetting.set(on);
	try {
		const res = await updateSettings({ appDeveloperMode: on });
		apply(res?.settings);
	} catch {
		await loadDevMode();
	}
}
