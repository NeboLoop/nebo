/**
 * App Update Store
 *
 * Tracks whether a new version of Nebo is available, download progress,
 * and whether the update is ready to apply.
 */

import { writable, derived } from 'svelte/store';

export interface UpdateState {
  available: boolean;
  ready: boolean;
  applying: boolean;
  error: string | null;
  currentVersion: string;
  latestVersion: string;
  canAutoUpdate: boolean;
  downloadPercent: number;
}

const initial: UpdateState = {
  available: false,
  ready: false,
  applying: false,
  error: null,
  currentVersion: '',
  latestVersion: '',
  canAutoUpdate: false,
  downloadPercent: 0,
};

export const updateState = writable<UpdateState>(initial);

/** True when we should show the banner (update ready to apply) */
export const showUpdateBanner = derived(updateState, ($s) => $s.ready && !$s.applying);

/** True when download is in progress */
export const updateDownloading = derived(updateState, ($s) => $s.available && !$s.ready && $s.downloadPercent > 0 && $s.downloadPercent < 100);

/**
 * Manually check for updates (tray menu, app menu, Settings → About).
 * Feeds the same update store the background checker uses — one pathway.
 */
export async function checkForUpdates(): Promise<'latest' | 'available' | 'error'> {
  const api = await import('$lib/api/nebo');
  const { addToast } = await import('$lib/stores/toast');
  try {
    const result = (await api.updateCheck()) as Record<string, unknown>;
    if (result?.available) {
      onUpdateAvailable(result);
      return 'available';
    }
    addToast("You're on the latest version", 'info');
    return 'latest';
  } catch {
    addToast('Failed to check for updates', 'error');
    return 'error';
  }
}

export function onUpdateAvailable(data: Record<string, unknown>) {
  updateState.update((s) => ({
    ...s,
    available: true,
    latestVersion: String(data.latestVersion || ''),
    currentVersion: String(data.currentVersion || s.currentVersion),
    canAutoUpdate: Boolean(data.canAutoUpdate),
    error: null,
  }));
}

export function onUpdateProgress(data: Record<string, unknown>) {
  updateState.update((s) => ({
    ...s,
    downloadPercent: Number(data.percent || 0),
  }));
}

export function onUpdateReady(_data: Record<string, unknown>) {
  updateState.update((s) => ({
    ...s,
    ready: true,
    downloadPercent: 100,
    error: null,
  }));
}

export function onUpdateError(data: Record<string, unknown>) {
  updateState.update((s) => ({
    ...s,
    error: String(data.error || data.message || 'Update failed'),
  }));
}

export function setApplying() {
  updateState.update((s) => ({ ...s, applying: true, error: null }));
}
