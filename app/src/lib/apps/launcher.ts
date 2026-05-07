/**
 * App launcher — opens an app in a new Tauri webview window or browser tab.
 * All apps pop out — they never render inline.
 *
 * Works in both Tauri desktop and headless/VPS (browser tab fallback).
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
 * In browser / VPS: opens a new tab.
 */
export async function launchApp(
	agentId: string,
	appName: string,
	config?: Partial<AppWindowConfig>
): Promise<void> {
	const cfg = { ...DEFAULT_CONFIG, ...config, title: config?.title || appName };
	const url = `/apps/${agentId}/ui/index.html`;

	// Tauri desktop — use WebviewWindow if available
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	if ((window as any).__TAURI_INTERNALS__) {
		try {
			const mod = await import('@tauri-apps/api/webviewWindow');
			const label = `app-${agentId}`;
			const existing = mod.WebviewWindow.getByLabel(label);
			if (await existing) {
				(await existing).setFocus();
				return;
			}
			new mod.WebviewWindow(label, {
				url,
				title: cfg.title,
				width: cfg.width,
				height: cfg.height,
				resizable: cfg.resizable
			});
			return;
		} catch {
			// Tauri API unavailable — fall through
		}
	}

	window.open(url, `app-${agentId}`);
}
