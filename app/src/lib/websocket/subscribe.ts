import { onDestroy } from 'svelte';
import { getWebSocketClient } from './client';

/**
 * Idiomatic Svelte subscription to a WebSocket event.
 *
 * There is ONE pathway for WS events: the singleton client's emitter
 * (`getWebSocketClient().on`). Components subscribe through this helper instead
 * of the old `window`-CustomEvent bridge — call it once during component init
 * and the handler is unsubscribed automatically on destroy. No manual
 * addEventListener/removeEventListener bookkeeping, and no leaked or duplicate
 * handlers across remounts/HMR (which is what caused the same event to fire its
 * side effect more than once).
 *
 *   onWsEvent('dep_installed', (data) => { ... });
 *
 * For a subscription that must re-bind when reactive state changes (e.g. a route
 * section), don't use this — use `$effect` directly, whose return value is the
 * cleanup the emitter hands back:
 *
 *   $effect(() => getWebSocketClient().on('plugin_auth_complete', handler));
 */
export function onWsEvent<T = unknown>(event: string, handler: (data: T) => void | Promise<void>): void {
	const off = getWebSocketClient().on<T>(event, handler);
	onDestroy(off);
}
