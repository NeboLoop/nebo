import { describe, expect, it } from 'vitest';
import type { PermissionsPage, PermissionSwitch } from '$lib/api/neboComponents';
import {
	allToolsEdits,
	allToolsValue,
	connectorIconUrl,
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

const page = (rows: PermissionSwitch[], def = 'allow'): PermissionsPage => ({
	mode: 'automatic',
	modeFromCompany: false,
	companyMode: 'automatic',
	capabilities: [],
	groups: [{ id: 'group:mcp:edge', title: 'Edge', subtitle: '', default: sw('tool:mcp__edge__*', def), rows }],
	specific: [],
	money: [],
	folders: [],
	alwaysAsks: [],
	fixed: []
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
		expect(connectorIconUrl('https://edge.example.test/mcp')).toBe('https://edge.example.test/favicon.ico');
		expect(connectorIconUrl('http://localhost:9/mcp')).toBeNull();
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
