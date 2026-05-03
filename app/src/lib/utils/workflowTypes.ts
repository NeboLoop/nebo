/**
 * Workflow Node Type System
 *
 * Follows v1's node-type methodology: each node type carries its own
 * default configuration, parameters, visual identity, and behavior.
 *
 * Categories:
 * - Activities: custom, research, email, notify, code
 * - Flow control: condition, loop, wait
 * - Integrations: connector (MCP), http
 * - Composition: agent (delegation), transform
 */

// ── Node type identifiers ────────────────────────────────────────────
export type ActivityType =
	| 'custom'
	| 'research'
	| 'email'
	| 'notify'
	| 'code'
	| 'condition'
	| 'loop'
	| 'wait'
	| 'agent'
	| 'connector'
	| 'http'
	| 'transform';

// ── Type definition ──────────────────────────────────────────────────
export interface ActivityTypeDefinition {
	type: ActivityType;
	label: string;
	description: string;
	icon: string;
	/** DaisyUI semantic color for the type accent on canvas */
	accentClass: string;
	/** Default skills pre-populated when creating this type */
	defaultSkills: string[];
	/** Default steps template */
	defaultSteps: string[];
	/** Type-specific parameter fields (beyond intent/steps/skills) */
	parameters: ActivityParameter[];
	/** If true, this node type creates branching outputs (e.g., condition, loop) */
	branches?: boolean;
	/** Output port labels for branching nodes */
	branchLabels?: string[];
}

export interface ActivityParameter {
	key: string;
	label: string;
	type: 'text' | 'textarea' | 'select' | 'number' | 'toggle';
	placeholder?: string;
	description?: string;
	options?: Array<{ value: string; label: string }>;
	default?: string | number | boolean;
}

// ── Type definitions ─────────────────────────────────────────────────
export const ACTIVITY_TYPES: Record<ActivityType, ActivityTypeDefinition> = {
	custom: {
		type: 'custom',
		label: 'Custom',
		description: 'Define steps and skills manually',
		icon: '◆',
		accentClass: 'border-base-300',
		defaultSkills: [],
		defaultSteps: [],
		parameters: [],
	},
	research: {
		type: 'research',
		label: 'Research',
		description: 'Web search, analysis, and synthesis',
		icon: '⊕',
		accentClass: 'border-success',
		defaultSkills: ['@nebo/skills/web-scraper@^1.0.0'],
		defaultSteps: [
			'Search for relevant sources',
			'Extract key findings',
			'Synthesize into summary',
		],
		parameters: [
			{
				key: 'depth',
				label: 'Research depth',
				type: 'select',
				options: [
					{ value: 'quick', label: 'Quick (2-5 min)' },
					{ value: 'standard', label: 'Standard (5-10 min)' },
					{ value: 'deep', label: 'Deep (10-20 min)' },
				],
				default: 'standard',
			},
			{
				key: 'sources',
				label: 'Source types',
				type: 'text',
				placeholder: 'web, academic, news',
				description: 'Comma-separated source types to search',
			},
		],
	},
	email: {
		type: 'email',
		label: 'Send Email',
		description: 'Compose and send email',
		icon: '✉',
		accentClass: 'border-info',
		defaultSkills: ['@nebo/skills/gws-gmail@^1.0.0'],
		defaultSteps: [
			'Compose email content',
			'Send via configured email provider',
		],
		parameters: [
			{
				key: 'to',
				label: 'To',
				type: 'text',
				placeholder: 'recipient@example.com',
				description: 'Recipient email address (or use upstream data)',
			},
			{
				key: 'subject',
				label: 'Subject template',
				type: 'text',
				placeholder: 'Re: {{topic}}',
			},
		],
	},
	notify: {
		type: 'notify',
		label: 'Notify',
		description: 'Send notification to a channel',
		icon: '⊘',
		accentClass: 'border-warning',
		defaultSkills: ['@nebo/skills/slack@^1.0.0'],
		defaultSteps: [
			'Format notification message',
			'Send to configured channel',
		],
		parameters: [
			{
				key: 'channel',
				label: 'Channel',
				type: 'select',
				options: [
					{ value: 'slack', label: 'Slack' },
					{ value: 'email', label: 'Email' },
					{ value: 'webhook', label: 'Webhook' },
				],
				default: 'slack',
			},
			{
				key: 'target',
				label: 'Target',
				type: 'text',
				placeholder: '#general or user@email.com',
				description: 'Channel name, email, or webhook URL',
			},
		],
	},
	code: {
		type: 'code',
		label: 'Run Code',
		description: 'Execute a code snippet',
		icon: '⌘',
		accentClass: 'border-secondary',
		defaultSkills: ['@nebo/skills/sandbox@^1.0.0'],
		defaultSteps: [
			'Execute code in sandboxed environment',
			'Capture output and return results',
		],
		parameters: [
			{
				key: 'language',
				label: 'Language',
				type: 'select',
				options: [
					{ value: 'javascript', label: 'JavaScript' },
					{ value: 'python', label: 'Python' },
					{ value: 'typescript', label: 'TypeScript' },
					{ value: 'shell', label: 'Shell' },
				],
				default: 'javascript',
			},
			{
				key: 'code',
				label: 'Code',
				type: 'textarea',
				placeholder: '// Your code here',
				description: 'Code to execute',
			},
		],
	},
	condition: {
		type: 'condition',
		label: 'Condition',
		description: 'If/else branching',
		icon: '⑂',
		accentClass: 'border-accent',
		branches: true,
		branchLabels: ['True', 'False'],
		defaultSkills: [],
		defaultSteps: [],
		parameters: [
			{
				key: 'expression',
				label: 'Condition expression',
				type: 'text',
				placeholder: 'data.status === "approved"',
				description: 'JavaScript expression — truthy routes to True branch, falsy to False',
			},
			{
				key: 'mode',
				label: 'Evaluation mode',
				type: 'select',
				options: [
					{ value: 'expression', label: 'Expression' },
					{ value: 'contains', label: 'Data contains' },
					{ value: 'exists', label: 'Field exists' },
					{ value: 'regex', label: 'Regex match' },
				],
				default: 'expression',
			},
		],
	},
	loop: {
		type: 'loop',
		label: 'Loop',
		description: 'Iterate over a list of items',
		icon: '↻',
		accentClass: 'border-accent',
		branches: true,
		branchLabels: ['Each item', 'Done'],
		defaultSkills: [],
		defaultSteps: [],
		parameters: [
			{
				key: 'source',
				label: 'Items source',
				type: 'text',
				placeholder: 'data.items or data.emails',
				description: 'Path to the array to iterate over',
			},
			{
				key: 'maxIterations',
				label: 'Max iterations',
				type: 'number',
				default: 100,
				description: 'Safety limit to prevent infinite loops',
			},
		],
	},
	wait: {
		type: 'wait',
		label: 'Wait',
		description: 'Pause execution',
		icon: '⏸',
		accentClass: 'border-base-content/30',
		defaultSkills: [],
		defaultSteps: [],
		parameters: [
			{
				key: 'duration',
				label: 'Wait duration',
				type: 'select',
				options: [
					{ value: '5s', label: '5 seconds' },
					{ value: '30s', label: '30 seconds' },
					{ value: '1m', label: '1 minute' },
					{ value: '5m', label: '5 minutes' },
					{ value: '15m', label: '15 minutes' },
					{ value: '1h', label: '1 hour' },
					{ value: 'custom', label: 'Custom' },
				],
				default: '1m',
			},
			{
				key: 'waitUntil',
				label: 'Or wait until',
				type: 'text',
				placeholder: 'e.g. event:approval.received',
				description: 'Resume when this event fires (leave empty for duration-based)',
			},
		],
	},
	agent: {
		type: 'agent',
		label: 'Agent',
		description: 'Delegate to another agent',
		icon: '◉',
		accentClass: 'border-primary',
		defaultSkills: [],
		defaultSteps: [
			'Prepare context and instructions for delegated agent',
			'Execute via agent and collect results',
		],
		parameters: [
			{
				key: 'agentId',
				label: 'Agent',
				type: 'select',
				options: [],
				description: 'Agent to delegate this task to',
			},
			{
				key: 'instructions',
				label: 'Instructions',
				type: 'textarea',
				placeholder: 'Specific instructions for the delegated agent...',
				description: 'Override or supplement the agent\'s default persona',
			},
		],
	},
	connector: {
		type: 'connector',
		label: 'Connector',
		description: 'Use an MCP integration',
		icon: '⊞',
		accentClass: 'border-primary',
		defaultSkills: [],
		defaultSteps: [
			'Execute tool from connected MCP server',
			'Process and pass results downstream',
		],
		parameters: [
			{
				key: 'serverId',
				label: 'MCP Server',
				type: 'select',
				options: [],
				description: 'Connected MCP server to use',
			},
			{
				key: 'tool',
				label: 'Tool',
				type: 'text',
				placeholder: 'e.g. read_file, send_message',
				description: 'Which tool to invoke on the MCP server',
			},
			{
				key: 'input',
				label: 'Input',
				type: 'textarea',
				placeholder: '{ "path": "/docs/report.md" }',
				description: 'JSON input for the tool (supports {{data.field}} templates)',
			},
		],
	},
	http: {
		type: 'http',
		label: 'HTTP Request',
		description: 'Make an HTTP API call',
		icon: '⇄',
		accentClass: 'border-info',
		defaultSkills: [],
		defaultSteps: [
			'Send HTTP request',
			'Parse response and pass data downstream',
		],
		parameters: [
			{
				key: 'method',
				label: 'Method',
				type: 'select',
				options: [
					{ value: 'GET', label: 'GET' },
					{ value: 'POST', label: 'POST' },
					{ value: 'PUT', label: 'PUT' },
					{ value: 'PATCH', label: 'PATCH' },
					{ value: 'DELETE', label: 'DELETE' },
				],
				default: 'GET',
			},
			{
				key: 'url',
				label: 'URL',
				type: 'text',
				placeholder: 'https://api.example.com/data',
			},
			{
				key: 'body',
				label: 'Request body',
				type: 'textarea',
				placeholder: '{ "key": "value" }',
				description: 'JSON body for POST/PUT/PATCH requests',
			},
			{
				key: 'headers',
				label: 'Headers',
				type: 'textarea',
				placeholder: 'Authorization: Bearer {{secrets.api_key}}',
				description: 'One header per line (key: value)',
			},
		],
	},
	transform: {
		type: 'transform',
		label: 'Transform',
		description: 'Reshape or filter data',
		icon: '⊿',
		accentClass: 'border-secondary',
		defaultSkills: [],
		defaultSteps: [
			'Apply transformation to input data',
			'Output transformed result',
		],
		parameters: [
			{
				key: 'operation',
				label: 'Operation',
				type: 'select',
				options: [
					{ value: 'map', label: 'Map — transform each item' },
					{ value: 'filter', label: 'Filter — keep matching items' },
					{ value: 'reduce', label: 'Reduce — aggregate into one value' },
					{ value: 'pick', label: 'Pick — extract specific fields' },
					{ value: 'template', label: 'Template — format as text' },
				],
				default: 'pick',
			},
			{
				key: 'expression',
				label: 'Expression',
				type: 'textarea',
				placeholder: 'data.results.map(r => r.title)',
				description: 'Transformation expression',
			},
		],
	},
};

// ── Helpers ──────────────────────────────────────────────────────────

/** Map catalog item type to activity type */
export function catalogTypeToActivityType(catalogType: string): ActivityType {
	if (catalogType.startsWith('activity-')) {
		const sub = catalogType.replace('activity-', '');
		if (sub in ACTIVITY_TYPES) return sub as ActivityType;
	}
	if (catalogType.startsWith('agent-')) return 'agent';
	if (catalogType.startsWith('connector-')) return 'connector';
	if (catalogType.startsWith('flow-')) {
		const sub = catalogType.replace('flow-', '');
		if (sub in ACTIVITY_TYPES) return sub as ActivityType;
	}
	return 'custom';
}

/** Get the type definition, falling back to custom */
export function getActivityType(type: ActivityType | string | undefined): ActivityTypeDefinition {
	return ACTIVITY_TYPES[type as ActivityType] || ACTIVITY_TYPES.custom;
}

/** Check if a type creates branching outputs */
export function isBranchingType(type: ActivityType | string | undefined): boolean {
	const def = getActivityType(type);
	return def.branches === true;
}

/** Create a new activity object with type-specific defaults */
export function createTypedActivity(
	catalogType: string,
	catalogItem: { label: string; desc: string; agentId?: string; serverId?: string; serverName?: string },
): { id: string; type: ActivityType; intent: string; skills: string[]; steps: string[]; params: Record<string, any> } {
	const actType = catalogTypeToActivityType(catalogType);
	const typeDef = ACTIVITY_TYPES[actType];
	const id = catalogItem.label.toLowerCase().replace(/\s+/g, '-') + '-' + Date.now().toString(36);

	const params: Record<string, any> = {};
	for (const p of typeDef.parameters) {
		if (p.default !== undefined) {
			params[p.key] = p.default;
		}
	}

	// Agent delegation: set the agentId param
	if (actType === 'agent' && catalogItem.agentId) {
		params.agentId = catalogItem.agentId;
	}

	// MCP Connector: set the server
	if (actType === 'connector' && catalogItem.serverId) {
		params.serverId = catalogItem.serverId;
	}

	const isAgent = actType === 'agent';
	const isConnector = actType === 'connector';
	let intent = catalogItem.desc || typeDef.description;
	if (isAgent) intent = `Delegate to ${catalogItem.label}`;
	if (isConnector && catalogItem.serverName) intent = `Use ${catalogItem.serverName}`;

	return {
		id,
		type: actType,
		intent,
		skills: [...typeDef.defaultSkills],
		steps: [...typeDef.defaultSteps],
		params,
	};
}
