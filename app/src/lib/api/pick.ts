import * as api from './nebo';
import type * as components from './neboComponents';

/**
 * The native file and folder pickers. In the desktop app the window shows
 * them (Tauri commands `pick_files` / `pick_folder`): Nebo's engine is a
 * process with no window, and macOS shows no panel from one. Anywhere else
 * the server shows them (`/api/v1/files/pick`, `/files/pick-folder`).
 */

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const inDesktopApp = () => typeof window !== 'undefined' && !!(window as any).__TAURI_INTERNALS__;

export async function pickFiles(): Promise<components.PickFilesResponse> {
	if (!inDesktopApp()) return api.pickFiles();
	const { invoke } = await import('@tauri-apps/api/core');
	return { paths: await invoke<string[]>('pick_files') };
}

export async function pickFolder(): Promise<components.PickFolderResponse> {
	if (!inDesktopApp()) return api.pickFolder();
	const { invoke } = await import('@tauri-apps/api/core');
	return { path: (await invoke<string | null>('pick_folder')) ?? '' };
}
