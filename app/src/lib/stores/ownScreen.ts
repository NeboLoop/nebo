import { readable } from 'svelte/store';
import { getSystemInfo } from '$lib/api/nebo';

/**
 * Whether this bot runs on a computer with a screen of its own (a desktop the
 * owner sits at) rather than in the cloud or on a headless server. Read once
 * from the bot; null until it answers. It never changes while the bot runs.
 */
let known: boolean | null = null;

export const ownScreen = readable<boolean | null>(known, (set) => {
	if (known !== null) {
		set(known);
		return;
	}
	getSystemInfo()
		.then((r) => {
			if (typeof r?.ownScreen === 'boolean') {
				known = r.ownScreen;
				set(known);
			}
		})
		.catch(() => {
			// Unknown stays unknown: the header behaves as it always did.
		});
});
