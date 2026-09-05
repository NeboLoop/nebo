import { describe, expect, it } from 'vitest';
import { formatServiceName, humanizeToolCall } from './humanize';

describe('humanizeToolCall', () => {
	it('humanizes STRAP signatures as verb + noun', () => {
		expect(humanizeToolCall('os', { resource: 'file', action: 'read' })).toEqual({
			label: 'reading file',
			outcome: 'Read file',
		});
		expect(humanizeToolCall('os', { resource: 'shell', action: 'exec' })).toEqual({
			label: 'running shell',
			outcome: 'Ran shell',
		});
	});

	it('names plugin calls by service, never "plugin"', () => {
		expect(humanizeToolCall('plugin', { resource: 'gws' })).toEqual({
			label: 'using Gws',
			outcome: 'Used Gws',
		});
	});

	it('humanizes MCP tools from slug + tool', () => {
		expect(humanizeToolCall('mcp__github__create_issue', {})).toEqual({
			label: 'using github (create issue)',
			outcome: 'Used github: create issue',
		});
	});
});

describe('formatServiceName', () => {
	it('title-cases slugs', () => {
		expect(formatServiceName('google-drive')).toBe('Google Drive');
		expect(formatServiceName('gws')).toBe('Gws');
	});
});