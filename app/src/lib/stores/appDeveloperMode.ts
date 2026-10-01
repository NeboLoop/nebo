import { writable } from 'svelte/store';
import { getSettings } from '$lib/api/nebo';

/**
 * App Developer mode, the bot's own setting (Settings > Developer). With it
 * on, an app's chat offers Publish. Read from the bot when a chat opens;
 * off until it answers, so a normal owner never sees developer controls.
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
