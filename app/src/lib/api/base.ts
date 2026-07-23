/**
 * Base URL of the Nebo backend that serves this SPA.
 *
 * Normally that is the page origin (desktop localhost, or the cloud pod
 * itself). When the SPA is opened through the loop's management tunnel it is
 * served under `/t/<botID>/` on the hub's origin — the bot's backend is only
 * reachable under that same prefix, so API and WS URLs must carry it or they
 * would hit the hub's own API instead of the bot's.
 *
 * The ONE place backend base resolution lives — REST, uploads, health, and
 * the WebSocket URLs all derive from here.
 */
import { base } from '$app/paths';

export function backendBase(): string {
	if (typeof window === 'undefined') return '';
	// Decision: SvelteKit's runtime-injected `base` is the source of truth.
	// The tunnel hub rewrites `base: ""` → `base: "/t/<botID>"` in the served
	// shell (neboloop tunnel.go ModifyResponse), and $lib/nav.ts already
	// trusts that value — deriving from the same source here means navigation
	// and API/WS resolution can never split-brain. The pathname regex remains
	// only as a fallback for a shell whose HTML re-rooting didn't run; on
	// desktop both branches yield the bare origin.
	if (base) return window.location.origin + base;
	const m = window.location.pathname.match(/^\/t\/[0-9a-fA-F-]{36}/);
	return window.location.origin + (m ? m[0] : '');
}

/**
 * ws(s):// form of backendBase() — chat and voice WebSocket URLs derive from
 * here (append the endpoint path, e.g. `/ws` or `/ws/voice/dictation`).
 */
export function backendWsBase(): string {
	const b = backendBase();
	if (!b) return '';
	return (b.startsWith('https:') ? 'wss:' : 'ws:') + b.replace(/^https?:/, '');
}

/**
 * Resolve a backend-served root-relative URL (e.g. an artifact's
 * `/api/v1/files/...`) against backendBase(). Absolute URLs pass through.
 */
export function backendUrl(url: string): string {
	return url.startsWith('/') && !url.startsWith('//') ? backendBase() + url : url;
}

/**
 * The ONE backend health probe. Resolves against backendBase() so it reaches
 * the bot's backend over the tunnel (a bare fetch('/health') would hit the
 * hub's own /health and lie). Returns the parsed body on 200, null when the
 * backend is unreachable or unhealthy.
 */
export async function backendHealth(): Promise<{ status?: string; version?: string } | null> {
	try {
		const resp = await fetch(`${backendBase()}/health`);
		if (!resp.ok) return null;
		return await resp.json();
	} catch {
		return null;
	}
}
