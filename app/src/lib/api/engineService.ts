/**
 * How the desktop app runs Nebo's engine (Tauri commands `engine_service`,
 * `set_start_at_login`, `open_login_items`): as the OS's service, kept
 * running after the app quits, or only while the app is open. Outside the
 * desktop app there is nothing to show: every call answers null.
 */

export interface EngineService {
	/** The OS service is on offer: Settings shows Start at login. */
	offered: boolean;
	startAtLogin: boolean;
	/** The OS runs the engine. */
	supervised: boolean;
	/** Nebo is switched off in Login Items: it runs only while the app is open. */
	needsApproval: boolean;
	/** The service was asked for but this computer can't run it (Linux with no systemd user session). */
	unavailable: boolean;
	/** "Keep running after I log out" (Linux); null where the OS has no such choice. */
	keepAfterLogout: boolean | null;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const inDesktopApp = () => typeof window !== 'undefined' && !!(window as any).__TAURI_INTERNALS__;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
	if (!inDesktopApp()) return null;
	const { invoke } = await import('@tauri-apps/api/core');
	try {
		return await invoke<T>(cmd, args);
	} catch {
		return null;
	}
}

export const engineService = () => invoke<EngineService>('engine_service');

export const setStartAtLogin = (on: boolean) => invoke<EngineService>('set_start_at_login', { on });

export const setKeepAfterLogout = (on: boolean) => invoke<EngineService>('set_keep_after_logout', { on });

export const openLoginItems = () => invoke<void>('open_login_items');
