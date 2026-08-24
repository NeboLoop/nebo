package main

// typeOverrides maps handler_name.key → TypeScript type.
// Used for response fields the generator can't infer automatically
// (ad-hoc json! objects, transformed collections, etc.).
//
// To add a new override:
//  1. Find the handler function name (e.g. list_agent_chats)
//  2. Find the response key (e.g. chats)
//  3. Add the mapping: "list_agent_chats.chats": "EnrichedChat[]"
//  4. If the type isn't in neboComponents.ts, add it to extraInterfaces below.
var typeOverrides = map[string]string{
	// ── Agent chats (enriched with preview, message count, relative time) ──
	"list_agent_chats.chats": "EnrichedChat[]",

	// ── Active agents ──
	"get_active_agents.agents": "ActiveAgent[]",

	// ── Agent runs ──
	"list_agent_runs.runs":  "AgentRunEntry[]",
	"list_agent_runs.total": "number",

	// ── Commander org chart ──
	"get_commander_org.nodes":      "CommanderNode[]",
	"get_commander_org.edges":      "CommanderEdge[]",
	"get_commander_org.teams":      "CommanderTeam[]",
	"get_commander_org.nodePositions": "CommanderNodePosition[]",

	// ── Chat messages ──
	"get_chat_messages.messages": "ChatMessage[]",
	"list_chat_messages.messages": "ChatMessage[]",
	// Same rows, addressed by session key (embed chat, coworker transcripts).
	"get_session_messages.messages": "ChatMessage[]",

	// ── Agents roster (enriched rows: display name, source, isolation, setup) ──
	"list_agents.agents": "AgentListEntry[]",
	"list_agents.primaryChristened": "boolean",

	// ── Workrooms (Workroom/WorkroomMessage generated from Rust structs) ──
	"list_workrooms.workrooms":        "Workroom[]",
	"create_workroom.workroom":        "Workroom",
	"get_workroom_messages.messages":  "WorkroomMessage[]",

	// ── User profile ──
	"userGetProfile.profile": "UserProfileFull",

	// ── User permissions ──
	"userGetPermissions.permissions":      "ToolPermission[]",
	"userGetPermissions.capabilities":     "Capability[]",
	"userGetPermissions.approvedCommands": "string[]",

	// ── Agent workflows (map keyed by binding name, NOT an array) ──
	"list_agent_workflows.workflows": "Record<string, AgentWorkflowEntry>",

	// ── Memories (Memory is already generated from the Rust struct) ──
	"list_memories.memories": "Memory[]",

	// ── Event sources (emit + watch auto-emissions, for trigger suggestions) ──
	"list_event_sources.sources": "EventSourceOption[]",

	// ── Published workflow endpoint (passthrough of NeboLoop's webhook JSON) ──
	"publish_agent_workflow.id":           "string",
	"publish_agent_workflow.agentId":      "string",
	"publish_agent_workflow.label":        "string",
	"publish_agent_workflow.workflowName": "string",
	"publish_agent_workflow.key":          "string",
	"publish_agent_workflow.keyPrefix":    "string",
	"publish_agent_workflow.url":          "string",

	// ── Misc ──
	"get_agent_stats.stats":       "AgentStats",
	"list_aliases.aliases":        "AliasEntry[]",
	"get_permissions.permissions": "ToolPermission[]",
}

// extraInterfaces defines TypeScript interfaces that don't exist as Rust structs
// but are needed by the type overrides above.
var extraInterfaces = map[string]string{
	"AgentListEntry": `export interface AgentListEntry {
	id: string
	name: string
	displayName: string
	description: string
	color?: string
	handle?: string
	source: string
	version?: string
	isApp: boolean
	isEnabled: boolean
	inputValues: string
	installedAt?: number
	loopExposed: boolean
	loopAgentId?: string
	voice: string
	isolated: boolean
	needsSetup: boolean
	nappPath?: string
	appWindowConfig?: AppWindowConfig
	loadError?: string
}`,

	"AppWindowConfig": `export interface AppWindowConfig {
	width: number
	height: number
	resizable: boolean
	title?: string
}`,

	"EnrichedChat": `export interface EnrichedChat {
	id: string
	name: string
	title: string
	preview: string
	updatedAt: string
	messages: number
	createdAt: number
	updatedAtEpoch: number
	sessionName: string
}`,

	"ActiveAgent": `export interface ActiveAgent {
	id: string
	agentId: string
	name: string
	status: string
}`,

	"Capability": `export interface Capability {
	key: string
	label: string
	desc: string
}`,

	"AgentRunEntry": `export interface AgentRunEntry {
	id: string
	name: string
	status: string
	duration: string
	date: string
	workflowRunId?: string
	trigger?: string
}`,

	"AgentStats": `export interface AgentStats {
	totalRuns: number
	completed: number
	failed: number
	running: number
	avgDuration: string
	lastRunAt: string
}`,

	"AliasEntry": `export interface AliasEntry {
	alias: string
	command: string
}`,

	"ToolPermission": `export interface ToolPermission {
	tool: string
	action: string
	allowed: boolean
}`,

	"CommanderNode": `export interface CommanderNode {
	id: string
	agentId: string
	name: string
	role: string
	type: string
	parentId?: string
}`,

	"EventSourceOption": `export interface EventSourceOption {
	value: string
	label: string
	kind: string
	agentName: string
	bindingName: string
	description?: string
}`,

	"AgentWorkflowTrigger": `export interface AgentWorkflowTrigger {
	type: string
	cron?: string
	schedule?: string
	interval?: string
	window?: { start?: string; end?: string }
	sources?: string[]
	event?: string
	plugin?: string
	command?: string
}`,

	"AgentWorkflowEntry": `export interface AgentWorkflowEntry {
	trigger: AgentWorkflowTrigger
	type?: string
	description?: string
	isActive: boolean
	lastFired?: string
	emit?: string
	activities?: unknown[]
	connections?: unknown[]
	inputs?: unknown
}`,

	"ImportItem": `export interface ImportItem {
	kind: 'mcp_server' | 'skill' | 'agent' | 'memory' | 'session' | 'cron' | 'credential'
	tier: 'content' | 'code' | 'reference'
	name: string
	detail: string
	target: string
	sourcePath: string
}`,

	"ImportManifest": `export interface ImportManifest {
	source: 'hermes' | 'openclaw'
	root: string
	items: ImportItem[]
	notes: string[]
}`,

	"ImportOutcome": `export interface ImportOutcome {
	agents: number
	skills: number
	mcpServers: number
	authProfiles: number
	memories: number
	chats: number
	chatMessages: number
	agentId: string | null
	agentName: string | null
	skipped: string[]
}`,

	"UserProfileFull": `export interface UserProfileFull {
	userId: string
	displayName?: string
	bio?: string
	location?: string
	timezone?: string
	occupation?: string
	interests?: string
	communicationStyle?: string
	goals?: string
	context?: string
	onboardingCompleted: boolean
	onboardingStep?: number
	toolPermissions?: string
	termsAcceptedAt?: number
	accountType?: string
	createdAt: number
	updatedAt: number
}`,

}
