import { writable } from 'svelte/store';

export interface UpdateInfo {
	available: boolean;
	current_version: string;
	latest_version: string;
	release_url: string;
	release_notes: string;
	published_at: string;
}

export const updateInfo = writable<UpdateInfo | null>(null);
export const updateDismissed = writable(false);

export async function checkForUpdate() {
	try {
		const res = await fetch('/api/v1/update/check');
		if (!res.ok) return;
		const data: UpdateInfo = await res.json();
		updateInfo.set(data);
	} catch {
		// Silently ignore — update check is non-critical
	}
}
