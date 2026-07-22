/**
 * Base URL of the Nebo backend that serves this SPA.
 *
 * Normally that is the page origin (desktop localhost, or the cloud pod
 * itself). When the SPA is opened through the loop's management tunnel it is
 * served under `/t/<botID>/` on the hub's origin — the bot's backend is only
 * reachable under that same prefix, so API and WS URLs must carry it or they
 * would hit the hub's own API instead of the bot's.
 *
 * The ONE place backend base resolution lives — REST, uploads, and the
 * WebSocket URL all derive from here.
 */
export function backendBase(): string {
	if (typeof window === 'undefined') return '';
	const m = window.location.pathname.match(/^\/t\/[0-9a-fA-F-]{36}/);
	return window.location.origin + (m ? m[0] : '');
}
