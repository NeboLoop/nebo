/**
 * App launcher — opens an app in a new Tauri webview window or browser popup.
 * All apps pop out — they never render inline.
 *
 * Works in both Tauri desktop and headless/VPS (browser popup fallback).
 */

export interface AppWindowConfig {
	width: number;
	height: number;
	resizable: boolean;
	title?: string;
}

const DEFAULT_CONFIG: AppWindowConfig = {
	width: 1024,
	height: 768,
	resizable: true
};

/**
 * Launch an app in a new window.
 * In Tauri: opens a WebviewWindow.
 * In browser / VPS: opens a popup window.
 */
export async function launchApp(
	agentId: string,
	appName: string,
	config?: Partial<AppWindowConfig>
): Promise<void> {
	const cfg = { ...DEFAULT_CONFIG, ...config, title: config?.title || appName };

	// Tauri desktop — use WebviewWindow if available
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	if ((window as any).__TAURI_INTERNALS__) {
		try {
			const mod = await import('@tauri-apps/api/webviewWindow');
			const { invoke } = await import('@tauri-apps/api/core');
			const label = `app-${agentId}`;
			const existing = await mod.WebviewWindow.getByLabel(label);
			if (existing) {
				try {
					await existing.setFocus();
					return;
				} catch {
					// Window was closed — destroy stale handle and create a new one
					try { await existing.destroy(); } catch { /* already gone */ }
				}
			}

			// Restore saved window size if the user previously resized this app
			const saved = await invoke<{ x: number; y: number; width: number; height: number } | null>(
				'get_window_state', { label }
			).catch(() => null);

			// Custom protocol: each app gets its own origin with / as root
			const appUrl = `neboapp://${agentId}/`;
			const wv = new mod.WebviewWindow(label, {
				url: appUrl,
				title: cfg.title,
				width: saved?.width ?? cfg.width,
				height: saved?.height ?? cfg.height,
				x: saved?.x,
				y: saved?.y,
				resizable: cfg.resizable
			});
			wv.once('tauri://error', (e) => {
				console.error('[launchApp] Tauri window error:', e);
			});
			return;
		} catch (err) {
			console.error('[launchApp] Failed to create Tauri window:', err);
			// Fall through to browser fallback
		}
	}

	// Browser fallback — open a popup window with app dimensions
	const left = Math.round((screen.width - cfg.width) / 2);
	const top = Math.round((screen.height - cfg.height) / 2);
	const features = `width=${cfg.width},height=${cfg.height},left=${left},top=${top},resizable=${cfg.resizable ? 'yes' : 'no'},scrollbars=yes`;
	window.open(`/apps/${agentId}/ui/`, `app-${agentId}`, features);
}
