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
	// ── Inbox counts: decisions still open, updates not yet read ──
	"unread_count.needsYou": "number",
	"unread_count.unread":   "number",

	// ── Teach a task ──
	"teach_start.capSeconds": "number",
	"teach_stop.keyframes":   "number",
	"teach_stop.narrated":    "boolean",
	"teach_stop.capped":      "boolean",

	// ── Agent chats (enriched with preview, message count, relative time) ──
	"list_agent_chats.chats":     "EnrichedChat[]",
	"list_agent_chats.teammates": "EnrichedChat[]",
	"list_agent_chats.pinned":    "EnrichedChat[]",
	"list_agent_chats.nextCursor": "string | null",
	"pin_chat.pinned":            "boolean",
	"pin_chat.pinnedAt":          "number | null",
	"edit_team.team":        "Team",

	// ── Providers: how a turn reaches each connection ──
	"list_providers.routes": "Record<string, ConnectionRoute>",

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
	// Linked bots (OpenClaw, Hermes) with chat, and the agents each serves:
	// what "Hire from <linked bot>" offers.
	"list_linked_agents.computers": "LinkedComputerEntry[]",
	// The coding apps this computer could host but does not have.
	"list_linked_agents.missing": "LinkedMissingApp[]",
	// When the owner's computers were last looked at (unix seconds).
	"list_linked_agents.checkedAt": "number",
	// Blank-create (the Hire flow) returns the introduction thread so the
	// UI can land the owner where the new employee is speaking.
	"create_agent.threadId": "string | null",
	// The one needs step: the plain line, and a draft only for an owner-made
	// job (a package's line needs none).
	"work_out_agent_needs.line": "string",
	"work_out_agent_needs.draftId": "string | null",

	// ── Run detail: human-readable projection derived server-side ──
	"get_run.display": "RunDisplay",

	// ── Teams (Team/TeamMessage generated from Rust structs) ──
	"list_teams.teams":            "RosterTeam[]",
	"open_team.team":              "Team",
	"get_team_messages.messages":  "TeamMessage[]",
	"send_team_message.asked":     "string[]",
	"edit_team.changed":           "boolean",
	"stop_team_work.stopped":      "string[]",
	"team_working.working":        "TeamWorkEntry[]",
	// A project's thread: the same rows, work list and unread entries as a team's.
	"project_working.working":     "TeamWorkEntry[]",
	"list_teams.unreadProjects":   "{ teamId: string; projectId: string }[]",

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

	// ── Model picker: provider id → that provider's models (a map, NOT an array) ──
	"list_models.models": "Record<string, unknown[]>",

	// ── Intelligence packs (Pack is generated from types::packs::Pack) ──
	// The bot's default: a pack ref `pack/<id>[/<level>]` or a model.
	"list_packs.default":         "string",
	"set_default_pack.default":   "string",
	"list_pack_choices.chat":     "PackChoice[]",
	"list_pack_choices.decision": "PackChoice[]",
	"list_pack_choices.voice":    "PackChoice[]",

	// ── A connection's models (ConnectionModel/CatalogModel are generated) ──
	"list_connection_models.models":         "ConnectionModel[]",
	"add_connection_model.model":            "ConnectionModel",
	"update_connection_model.model":         "ConnectionModel",
	"browse_connection_catalog.models":      "CatalogModel[]",
	"browse_connection_catalog.refreshedAt": "number | null",
	"browse_connection_catalog.error":       "'key' | 'rate_limited' | 'unreachable' | null",

	// ── Misc ──
	"get_agent_stats.stats":       "AgentStats",
	"list_aliases.aliases":        "AliasEntry[]",
	"get_permissions.permissions": "ToolPermission[]",
}

// extraInterfaces defines TypeScript interfaces that don't exist as Rust structs
// but are needed by the type overrides above.
var extraInterfaces = map[string]string{
	// How a turn reaches a connection (handlers/provider.rs connection_route).
	"ConnectionRoute": `export interface ConnectionRoute {
	via: 'janus' | 'direct'
	signInRequired: boolean
}`,
	// A file as it travels (comm::wire::Attachment), in an upload's status
	// (handlers/uploads.rs UploadStatus).
	"Attachment": `export interface Attachment {
	fileId: string
	filename: string
	mimeType: string
	size: number
	url: string
	thumbnailUrl?: string
	width?: number
	height?: number
	duration?: number
}`,

	// Where a landed file's hub copy stands (crates/server/src/hub_sync.rs).
	"SyncState": `export type SyncState = 'pending' | 'synced' | 'failed' | 'refused'`,

	// One piece of background work (crates/server/src/background.rs).
	"BackgroundTask": `export type BackgroundKind = 'agent' | 'shell' | 'workflow' | 'monitor' | 'loop'

export type BackgroundSource = 'helper' | 'turn' | 'workflow' | 'timer' | 'watch' | 'heartbeat' | 'shell' | 'handoff'

export type BackgroundStatus = 'running' | 'waiting' | 'scheduled' | 'watching' | 'degraded'

export type BackgroundAction = 'stop' | 'cancel' | 'approve' | 'pause' | 'delete' | 'off'

export interface BackgroundTask {
	id: string
	kind: BackgroundKind
	source: BackgroundSource
	agentId: string
	employee: string
	title: string
	detail: string
	call: CallName | null
	status: BackgroundStatus
	wait: string | null
	trigger: string | null
	startedAt: number | null
	lastActivityAt: number | null
	nextRunAt: number | null
	createdBy: string
	sourceRunId: string | null
	sessionKey: string | null
	fromAgentId: string | null
	fromSessionKey: string | null
	launchedFrom: string | null
	transcript: BackgroundTranscript | null
	turnsUsed: number | null
	turnsCap: number | null
	actions: BackgroundAction[]
}

export interface BackgroundTranscript {
	sessionKey: string
	chatId: string
}`,
	// A piece of background work that ended, and how.
	"FinishedTask": `export interface FinishedTask {
	task: BackgroundTask
	outcome: 'done' | 'failed' | 'stopped' | 'removed'
	endedAt: number
}`,
	// One row of a team's "who's working" strip (GET /teams/{id}/working).
	"TeamWorkEntry": `export interface TeamWorkEntry {
	kind: 'member' | 'helper'
	agentId: string
	member: string
	title: string
	taskId: string
	call: CallName | null
	activity: string
	sessionKey: string
	chatId: string
}`,
	// A tool call's structured name (crates/tools/src/humanize.rs Call):
	// a kind every client words in the owner's language, and the call's own
	// values verbatim.
	"CallName": `export interface CallName {
	kind: string
	params?: Record<string, string>
}`,
	"RunDisplay": `export interface RunDisplayFact {
	key: string
	value: string
}

export interface RunDisplayEntry {
	line: string | null
	verdict?: string
	facts: RunDisplayFact[]
}

export interface RunDisplay {
	input: RunDisplayEntry | null
	activities: Record<string, RunDisplayEntry>
}`,
	"AgentListEntry": `export interface AgentListEntry {
	id: string
	name: string
	displayName: string
	description: string
	/** Last visible message of the latest thread; the roster row's second line. */
	latestPreview?: string
	/** The latest thread was cut by a server restart; latestPreview is the status before it. */
	restarted?: boolean
	color?: string
	/** The character it wears: assistant for the primary, else a mountain shape (types::character). */
	shape: string
	handle?: string
	source: string
	version?: string
	isApp: boolean
	/** AGENT.md's starters: asks an empty chat offers to tap (at most 4). */
	starters: string[]
	isEnabled: boolean
	inputValues: string
	installedAt?: number
	loopExposed: boolean
	loopAgentId?: string
	voice: string
	/** The part of the company this employee sits in; unset = unassigned. */
	department?: string
	/** The employee this one answers to (local agent id); unset = answers to the owner. */
	reportsTo?: string
	/** Conversations are kept apart (memory.mode "separate" or "confidential"). */
	isolated: boolean
	/** memory.mode: "single", "separate" or "confidential". */
	memoryMode: string
	needsSetup: boolean
	nappPath?: string
	appWindowConfig?: AppWindowConfig
	/** Apps only: how the page asks to be shown (manifest window + device:motion). */
	appWindow?: AppWindow
	loadError?: string
	/** "linked" for an employee hired from a linked bot; unset otherwise. */
	kind?: string
	/** Linked employees only: the linked bot cannot be reached right now. */
	offline?: boolean
	/** Its place in the owner's Pinned list (employees and teams share it); null when not pinned. The rows come in the owner's order. */
	pinPosition: number | null
}`,

	// comm::api_types::FileShare — the hub's link for a shared file.
	"FileShare": `export interface FileShare {
	/** Only for changing or turning the link off; never shown. */
	id: string
	/** https://neboai.com/s/<token> */
	url: string
	filename: string
	/** link (anyone with the link), password, or private (only you). */
	access: 'link' | 'password' | 'private'
	hasPassword: boolean
	/** RFC 3339; empty = never. */
	expiresAt: string
	createdAt: string
	/** The Work-panel reference of the file. */
	source: string
	/** The link follows its file; otherwise it keeps the version it has. */
	live: boolean
	contentHash: string
	/** Published as a site: its address and https://<address>.nebo.page. */
	address?: string
	siteUrl?: string
}`,

	// One model a pack slot may name (GET /intelligence-packs/choices):
	// value is the provider/model string the slot stores.
	"PackChoice": `export interface PackChoice {
	value: string
	provider: string
	connection: string
	modelId: string
	/** What the model reads as when its provider names it (a Nebo AI speed: "High and Max"). */
	name?: string
	capabilities?: string[]
	local: boolean
	/** What the owner reads before picking it: what it is, what it is good for, its price. */
	card?: ModelCard
	/** One of the models Janus's catalog lists (not nebo-1 or a speed): chosen only by its full id. */
	catalog?: boolean
}`,

	"LinkedComputerEntry": `export interface LinkedComputerEntry {
	/** ` + "`computer:<hostname>`" + `: a key, never a bot. */
	id: string
	/** "Mac.lan"; "This computer" for the computer this bot runs on. */
	name: string
	online: boolean
	local: boolean
	/** What is installed on it, one entry per app, in the order shown. */
	agents: LinkedAgentEntry[]
}`,

	"LinkedMissingApp": `export interface LinkedMissingApp {
	/** The runtime it would run: "claude-code". */
	runtime: string
	/** The app as the owner knows it: "Claude Code". */
	app: string
	/** Where its makers say how to install it. */
	installUrl: string
}`,

	"LinkedAgentEntry": `export interface LinkedAgentEntry {
	/** The agent's id, or ` + "`new:<runtime>`" + ` for a coding employee to start. */
	id: string
	/** The agent's name; the employee's, once it is on the team. */
	name: string
	/** The app it is: "Claude Code", "Hermes". */
	app: string
	/** "Hermes on Mac.lan"; "Works in a new folder on Mac.lan". */
	description: string
	/** The runtime it runs: "hermes", "codex". */
	runtime: string
	/** The bot a hire of it goes to (` + "`linked.botId`" + `). */
	botId: string
	/** Already on the team: shown, not hired again. */
	hired: boolean
	/** The employee it is, once it is on the team. */
	employeeId: string | null
}`,

	"AppWindowConfig": `export interface AppWindowConfig {
	width: number
	height: number
	resizable: boolean
	title?: string
	/** The page takes the whole screen (a game). Unset = false. */
	fullscreen?: boolean
	/** Unset = portrait. */
	orientation?: 'portrait' | 'landscape' | 'any'
}`,

	// napp::manifest::AppWindow — `appWindow` on an app employee.
	"AppWindow": `export interface AppWindow {
	fullscreen: boolean
	orientation: 'portrait' | 'landscape' | 'any'
	/** The manifest declares device:motion. */
	motion: boolean
	/** window.open_on_work: the app opens over its chat when the employee writes that chat's record. */
	openOnWork: boolean
	/** window.isolated: the page is served cross-origin isolated (SharedArrayBuffer for threaded engine exports). */
	isolated: boolean
	/** window.share_menu: the header's Share button entries, each say sent into the app's chat; share: the answer's file opens in the Share dialog. */
	shareMenu: { label: string; say: string; share?: boolean }[]
}`,

	"EnrichedChat": `export interface EnrichedChat {
	id: string
	name: string
	title: string
	/** Whose conversation it is: the owner's, or the employee's thread with a colleague or a team. */
	kind: 'owner' | 'colleague' | 'team'
	/** Who is on the other side of a colleague or team thread; null for the owner's. */
	with: string | null
	preview: string
	updatedAt: string
	messages: number
	createdAt: number
	updatedAtEpoch: number
	sessionName: string
	/** Whether the owner pinned it to the top of the employee's list. */
	pinned: boolean
	/** When he pinned it (unix seconds); null when not pinned. */
	pinnedAt: number | null
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
	audit?: WorkflowAudit
}`,

	"WorkflowAudit": `export interface WorkflowAudit {
	definitionHash: string
	depth: 'programmatic' | 'full'
	steps: { step: string; added: { tool: string; source: 'history' | 'named' | 'declared' | 'model' }[]; resolvedBy: 'history' | 'named' | 'declared' | 'model' | null }[]
	fixes: { step: string; message: string }[]
	unresolved: string[]
	issues: { severity: 'error' | 'warning'; workflow: string; step: string | null; message: string }[]
	disconnected: string[]
	modelCalls: number
	healed: { step: string; tool: string; readOnly: boolean; outcome: 'ran' | 'asked'; at: number }[]
	auditedAt: number
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
