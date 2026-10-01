import { writable } from 'svelte/store';
import { getSettings } from '$lib/api/nebo';

/**
 * App Developer mode, the bot's own setting (Settings > Developer): app
 * employees work on each other's apps, and app pages show the floating
 * console. Off until the bot answers, so a normal owner never sees
 * developer controls.
 */
export const appDeveloperMode = writable(false);

export async function loadAppDeveloperMode(): Promise<void> {
	try {
		const res = await getSettings();
		appDeveloperMode.set(!!res?.settings?.appDeveloperMode);
	} catch {
		// Keep what was known: a failed read never shows the button.
	}
}
