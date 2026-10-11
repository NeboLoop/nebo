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

/** Where a connector's icon may be, in order: its host's favicon, then
 *  its site's (`mcp.example.com` → `example.com`). Only over https. */
export function connectorIconUrls(serverUrl: string): string[] {
	try {
		const u = new URL(serverUrl);
		if (u.protocol !== 'https:') return [];
		const labels = u.hostname.split('.');
		const urls = [`${u.origin}/favicon.ico`];
		if (labels.length > 2) urls.push(`https://${labels.slice(-2).join('.')}/favicon.ico`);
		return urls;
	} catch {
		return [];
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

/** A row on a connector's view: one tool, or the tools that share a first
 *  word in their name (`calendar_list`, `calendar_create` → "Calendar"),
 *  set together. Grouped only when that makes more than one row. */
export interface ToolGroup<T extends { key: string; title: string }> {
	title: string;
	tools: T[];
}

export function groupTools<T extends { key: string; title: string }>(tools: T[]): ToolGroup<T>[] {
	const word = (t: T) => (t.key.includes('__') ? t.key.split('__').pop()! : t.key).split(/[_\-.]/)[0];
	const byWord = new Map<string, T[]>();
	for (const t of tools) byWord.set(word(t), [...(byWord.get(word(t)) ?? []), t]);
	const grouping = byWord.size > 1 && [...byWord.values()].some((g) => g.length > 1);
	if (!grouping) return tools.map((t) => ({ title: t.title, tools: [t] }));
	return [...byWord.entries()]
		.map(([w, g]) => ({ title: g.length === 1 ? g[0].title : w.charAt(0).toUpperCase() + w.slice(1), tools: g }))
		.sort((a, b) => a.title.toLowerCase().localeCompare(b.title.toLowerCase()));
}

/** Several tools' common setting, or `custom` when they differ. */
export function groupValue(switches: Map<string, PermissionSwitch>, family: string, keys: string[]): string {
	const values = new Set(keys.map((k) => toolValue(switches, family, k)));
	return values.size === 1 ? [...values][0] : 'custom';
}

/** A tool's description as a footnote: its first paragraph. */
export function toolSummary(description: string): string {
	const first = description.trim().split(/\n\s*\n/)[0].trim();
	return first.length > 400 ? `${first.slice(0, 400).trimEnd()}…` : first;
}
