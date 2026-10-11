import { describe, expect, it } from 'vitest';
import type { ConnectorToolsResponse, PermissionSwitch } from '$lib/api/neboComponents';
import {
	groupTools,
	toolSummary,
	allToolsEdits,
	allToolsValue,
	connectorIconUrls,
	connectorState,
	switchesById,
	toolValue,
	validConnectorUrl,
	type Connector
} from './connectors';

const row = (patch: Partial<Connector> = {}): Connector => ({
	id: 'c1',
	name: 'Edge',
	serverUrl: 'https://edge.example.test/mcp',
	authType: 'oauth',
	isEnabled: true,
	connectionStatus: 'connected',
	toolCount: 2,
	lastError: null,
	...patch
});

const sw = (id: string, value: string, own = false): PermissionSwitch => ({
	id,
	sentence: id,
	value,
	inherited: !own,
	canInherit: own,
	locked: false
});

const page = (rows: PermissionSwitch[], def = 'allow'): ConnectorToolsResponse => ({
	family: 'mcp__edge__*',
	default: sw('tool:mcp__edge__*', def),
	tools: rows.map((setting) => {
		const key = setting.id.slice('tool:'.length);
		return { name: key.split('__').pop()!, key, title: key, description: '', setting };
	})
});

describe('connectorState', () => {
	it('reads the bot and an open sign-in', () => {
		expect(connectorState(row())).toBe('connected');
		expect(connectorState(row({ connectionStatus: 'needs_reauth' }))).toBe('expired');
		expect(connectorState(row({ isEnabled: false, connectionStatus: 'disconnected' }))).toBe('off');
		expect(connectorState(row({ connectionStatus: 'error' }))).toBe('error');
		expect(connectorState(row({ connectionStatus: 'needs_reauth' }), 'waiting')).toBe('waiting');
		expect(connectorState(row(), 'timedOut')).toBe('timedOut');
	});
});

describe('connector address and icon', () => {
	it('takes https, or http on this computer', () => {
		expect(validConnectorUrl('https://mcp.example.com/mcp')).toBe(true);
		expect(validConnectorUrl('http://localhost:8080/mcp')).toBe(true);
		expect(validConnectorUrl('http://mcp.example.com/mcp')).toBe(false);
		expect(validConnectorUrl('mcp.example.com')).toBe(false);
	});
	it('shows the server favicon over https only', () => {
		expect(connectorIconUrls('https://mcp.edge.example/mcp')).toEqual(['https://mcp.edge.example/favicon.ico', 'https://edge.example/favicon.ico']);
		expect(connectorIconUrls('https://example.com/mcp')).toEqual(['https://example.com/favicon.ico']);
		expect(connectorIconUrls('http://localhost:9/mcp')).toEqual([]);
	});
});

describe('tool settings', () => {
	const family = 'mcp__edge__*';
	const keys = ['mcp__edge__logs', 'mcp__edge__nodes'];

	it('a tool follows the default unless it has its own; All tools says Custom when they differ', () => {
		const s = switchesById(page([sw('tool:mcp__edge__logs', 'allow'), sw('tool:mcp__edge__nodes', 'ask', true)]));
		expect(toolValue(s, family, 'mcp__edge__logs')).toBe('allow');
		expect(toolValue(s, family, 'mcp__edge__nodes')).toBe('ask');
		expect(allToolsValue(s, family, keys)).toBe('custom');
	});

	it('All tools moves the default and clears each tool of its own', () => {
		const s = switchesById(page([sw('tool:mcp__edge__logs', 'allow'), sw('tool:mcp__edge__nodes', 'ask', true)]));
		expect(allToolsEdits(s, family, keys, 'deny')).toEqual([
			{ id: 'tool:mcp__edge__*', value: 'deny' },
			{ id: 'tool:mcp__edge__nodes', value: 'inherit' }
		]);
	});

	it('all alike: the default is the summary', () => {
		const s = switchesById(page([sw('tool:mcp__edge__logs', 'ask'), sw('tool:mcp__edge__nodes', 'ask')], 'ask'));
		expect(allToolsValue(s, family, keys)).toBe('ask');
	});
});

describe('tool rows', () => {
	const t = (name: string, title = name) => ({ key: `mcp__crm__${name}`, title });
	it('groups tools that share a first word when that makes more than one row', () => {
		expect(groupTools([t('calendar_list'), t('calendar_create'), t('mail_send')]).map((g) => [g.title, g.tools.length])).toEqual([
			['Calendar', 2],
			['mail_send', 1]
		]);
	});
	it('otherwise lists each tool, by its own title', () => {
		expect(groupTools([t('memory_link'), t('memory_search')]).map((g) => g.title)).toEqual(['memory_link', 'memory_search']);
		expect(groupTools([t('agent', 'Agent Management'), t('catalog', 'Catalog Management')]).map((g) => g.title)).toEqual([
			'Agent Management',
			'Catalog Management'
		]);
	});
	it('a footnote is the first paragraph', () => {
		expect(toolSummary('Manage agents.\n\nActions:\n- list')).toBe('Manage agents.');
	});
});
