// Connectors: the MCP servers on this Nebo, as the Connectors page shows
// them. Pure helpers the page and its tests share.

import type { PermissionsPage, PermissionSwitch } from '$lib/api/neboComponents';

export interface Connector {
	id: string;
	name: string;
	serverUrl: string;
	/** oauth | api_key | none | neboai (Company Memory) */
	authType: string;
	isEnabled: boolean;
	/** The bot's word: connected | disconnected | error | needs_reauth */
	connectionStatus: string;
	toolCount: number;
	lastError: string | null;
}

/** Where a connector stands. `waiting` and `timedOut` are a sign-in this
 *  page opened: still open in the browser, or never finished. */
export type ConnectorState = 'connected' | 'off' | 'expired' | 'error' | 'waiting' | 'timedOut';

export function connectorState(c: Connector, signIn?: 'waiting' | 'timedOut'): ConnectorState {
	if (signIn) return signIn;
	if (c.connectionStatus === 'needs_reauth') return 'expired';
	if (!c.isEnabled) return 'off';
	if (c.connectionStatus === 'connected') return 'connected';
	if (c.connectionStatus === 'error') return 'error';
	return 'off';
}

/** A connector's icon: its server's favicon, when it is served over https. */
export function connectorIconUrl(serverUrl: string): string | null {
	try {
		const u = new URL(serverUrl);
		return u.protocol === 'https:' ? `${u.origin}/favicon.ico` : null;
	} catch {
		return null;
	}
}

/** An address a connector can be added at: https, or http on this computer. */
export function validConnectorUrl(raw: string): boolean {
	try {
		const u = new URL(raw.trim());
		if (u.protocol === 'https:') return !!u.hostname;
		return u.protocol === 'http:' && (u.hostname === 'localhost' || u.hostname === '127.0.0.1');
	} catch {
		return false;
	}
}

/** The company Permissions page's switches, by id (`tool:<key>`). */
export function switchesById(page: PermissionsPage): Map<string, PermissionSwitch> {
	const m = new Map<string, PermissionSwitch>();
	for (const g of page.groups) for (const s of [g.default, ...g.rows]) m.set(s.id, s);
	return m;
}

/** A tool's setting: its own switch, else its connector's default. */
export function toolValue(switches: Map<string, PermissionSwitch>, family: string, key: string): string {
	return switches.get(`tool:${key}`)?.value ?? switches.get(`tool:${family}`)?.value ?? 'allow';
}

/** "All tools": the default's value, or `custom` when a tool differs. */
export function allToolsValue(switches: Map<string, PermissionSwitch>, family: string, keys: string[]): string {
	const def = switches.get(`tool:${family}`)?.value ?? 'allow';
	return keys.some((k) => toolValue(switches, family, k) !== def) ? 'custom' : def;
}

/** The switch moves "All tools" makes: the default, then each tool's own
 *  setting cleared so it follows the default. */
export function allToolsEdits(switches: Map<string, PermissionSwitch>, family: string, keys: string[], value: string) {
	const edits = [{ id: `tool:${family}`, value }];
	for (const k of keys) {
		const s = switches.get(`tool:${k}`);
		if (s && !s.inherited && s.canInherit) edits.push({ id: s.id, value: 'inherit' });
	}
	return edits;
}
