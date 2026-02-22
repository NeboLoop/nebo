import { writable } from 'svelte/store';
import type { UpdateCheckResponse } from '$lib/api/neboComponents';
import * as api from '$lib/api/nebo';

export interface DownloadProgress {
	downloaded: number;
	total: number;
	percent: number;
}

export const updateInfo = writable<UpdateCheckResponse | null>(null);
export const updateDismissed = writable(false);
export const downloadProgress = writable<DownloadProgress | null>(null);
export const updateReady = writable<string | null>(null); // version string when ready
export const updateError = writable<string | null>(null);

export async function checkForUpdate() {
	try {
		const data = await api.updateCheck();
		if (data) {
			updateInfo.set(data);
		}
	} catch {
		// Silently ignore — update check is non-critical
	}
}

export async function applyUpdate() {
	try {
		await api.updateApply();
	} catch {
		// Server may have already restarted
	}
}

export function resetUpdateState() {
	downloadProgress.set(null);
	updateReady.set(null);
	updateError.set(null);
	updateDismissed.set(false);
}
