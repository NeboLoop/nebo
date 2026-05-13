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
	"list_agent_runs.runs": "AgentRunEntry[]",

	// ── Commander org chart ──
	"get_commander_org.nodes":      "CommanderNode[]",
	"get_commander_org.edges":      "CommanderEdge[]",
	"get_commander_org.teams":      "CommanderTeam[]",
	"get_commander_org.nodePositions": "CommanderNodePosition[]",

	// ── Extensions / skills ──
	"list_extensions.extensions": "ExtensionInfo[]",

	// ── Chat messages ──
	"get_chat_messages.messages": "ChatMessage[]",
	"list_chat_messages.messages": "ChatMessage[]",

	// ── User profile ──
	"userGetProfile.profile": "UserProfileFull",

	// ── User permissions ──
	"userGetPermissions.permissions": "ToolPermission[]",

	// ── Misc ──
	"get_agent_stats.stats":       "AgentStats",
	"list_aliases.aliases":        "AliasEntry[]",
	"get_permissions.permissions": "ToolPermission[]",
}

// extraInterfaces defines TypeScript interfaces that don't exist as Rust structs
// but are needed by the type overrides above.
var extraInterfaces = map[string]string{
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

	"AgentRunEntry": `export interface AgentRunEntry {
	id: string
	name: string
	status: string
	duration: string
	date: string
	workflowRunId?: string
	trigger?: string
}`,

	"ExtensionInfo": `export interface ExtensionInfo {
	name: string
	description: string
	version: string
	isEnabled: boolean
	type: string
	source: string
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
	createdAt: number
	updatedAt: number
}`,

}
