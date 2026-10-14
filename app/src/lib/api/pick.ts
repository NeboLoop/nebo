import * as api from './nebo';
import type * as components from './neboComponents';
import { inDesktopApp } from './engineService';

/**
 * The native file and folder pickers. In the desktop app the window shows
 * them (Tauri commands `pick_files` / `pick_folder`): Nebo's engine is a
 * process with no window, and macOS shows no panel from one. Anywhere else
 * the server shows them (`/api/v1/files/pick`, `/files/pick-folder`).
 */

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
