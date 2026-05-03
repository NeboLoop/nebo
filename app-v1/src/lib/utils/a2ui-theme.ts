/**
 * Dynamic theme CSS loader for A2UI workspaces.
 *
 * Fetches agent-specific theme.css and injects it into document.head as a
 * <style media="not all"> element — this makes it available for shadow DOM
 * cloning (via NeboSurfaceElement's MutationObserver) without affecting the
 * global page styles. The shadow root clone strips the media attribute so
 * the theme applies only inside the A2UI surface.
 */

const activeThemes = new Map<string, HTMLStyleElement>();

export async function loadAgentTheme(agentId: string): Promise<void> {
	if (activeThemes.has(agentId)) return;
	try {
		const res = await fetch(`/api/v1/agents/${encodeURIComponent(agentId)}/theme.css`);
		if (!res.ok) return;
		const css = await res.text();
		if (!css.trim()) return;
		const style = document.createElement('style');
		style.dataset.a2uiTheme = agentId;
		style.media = 'not all'; // Prevent global application; shadow root clones enable it
		style.textContent = css;
		document.head.appendChild(style);
		activeThemes.set(agentId, style);
	} catch {
		// Ignore fetch errors — agent may not have a theme
	}
}

export function unloadAgentTheme(agentId: string): void {
	const el = activeThemes.get(agentId);
	if (el) {
		el.remove();
		activeThemes.delete(agentId);
	}
}
