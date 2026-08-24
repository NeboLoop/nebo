// Types for the [agentId] layout context shared across child routes.

/** Input field configuration for agent setup forms. */
export interface AgentInputField {
	key: string
	label: string
	description?: string
	type: string
	required?: boolean
	default?: unknown
	placeholder?: string
	options?: { value: string; label: string }[]
}

/** Enriched chat object returned by list_agent_chats (not the raw Chat struct). */
export interface EnrichedChat {
	id: string
	name: string
	title: string
	preview: string
	updatedAt: string
	messages: number
	createdAt: number
	updatedAtEpoch: number
	sessionName: string
}

/** Workflow trigger configuration. */
export interface WorkflowTrigger {
	type: string
	/** Call trigger: which phone line's label this tree answers ('' = all). */
	line?: string
	event?: string
	sources?: string[]
	schedule?: string
	cron?: string
	interval?: string
	plugin?: string
	command?: string
	window?: { start?: string; end?: string }
}

/** Workflow object used in the agent config. */
export interface WorkflowConfig {
	/** Binding kind: undefined = standard workflow, 'call_tree' = phone-line config. */
	type?: string
	trigger?: WorkflowTrigger
	schedule?: string
	activities?: WorkflowActivity[]
	connections?: { from: string; to: string; label?: string }[]
	source?: string
	isActive?: boolean
	description?: string
	lastFired?: string
	emit?: string
}

/** A single workflow activity. */
export interface WorkflowActivity {
	id: string
	type: string
	label?: string
	description?: string
	tool?: string
	resource?: string
	action?: string
	params?: Record<string, unknown>
	branches?: { label: string; nextId?: string }[]
	intent?: string
	skills?: string[]
	steps?: string[]
}

/** Workflow stats for an agent. */
export interface WorkflowStatsLocal {
	totalRuns: number
	completed: number
	failed: number
	running: number
	avgDuration: string
	lastRunAt: string
}

/** Agent run entry. */
export interface AgentRun {
	id: string
	name: string
	/** Workflow binding name (e.g., "auto-reply", "day-monitor"). */
	workflowName: string
	status: string
	duration: string
	date: string
	dateGroup: string
	time: string
	workflowRunId?: string
	trigger?: string
	output?: string
	error?: string
}

/** Local agent display object (derived from API Agent). */
export interface AgentDisplay {
	/** memory.context_isolated — each conversation keeps its own sealed memory. */
	isolated?: boolean
	id: string
	name: string
	role: string
	initial: string
	status: string
	color: string
	/** User-editable handle, stored as `bot_<chosen>`. */
	handle?: string
	editable?: boolean
	isApp?: boolean
	/** Whether this agent is exposed on the user's personal loop. */
	loopExposed?: boolean
	/** Hub identity UUID — the wire's mention tokens may carry this id. */
	loopAgentId?: string
	/** Realtime voice id (xAI voice id; empty = default). */
	voice?: string
}

/** The agentPage context shape provided by [agentId]/+layout.svelte. */
export interface AgentPageContext {
	readonly agentId: string
	readonly agent: AgentDisplay | undefined
	readonly agentColor: Record<string, string> | null
	readonly threads: EnrichedChat[]
	readonly isThreadsLoading: boolean
	readonly agentsLoading: boolean
	readonly runs: AgentRun[]
	readonly runsTotal: number
	readonly hasMoreRuns: boolean
	readonly runsLoading: boolean
	loadMoreRuns: () => Promise<void>
	readonly skills: string[]
	readonly config: { persona: string; agentMd: string; soul: string; rules: string; model: string; inputs: unknown[]; workflows: Record<string, WorkflowConfig> }
	readonly workflowEntries: [string, WorkflowConfig][]
	readonly workflowStats: WorkflowStatsLocal
	readonly workflowRuns: unknown[]
	readonly isApp: boolean
	readonly devMode: boolean
	readonly agentStatuses: Record<string, string>
	openWorkflow: (name: string, wf: WorkflowConfig) => void
	/** Open the runs modal. */
	openRuns: () => void
	/** Open this employee's settings modal. */
	openSettings: () => void
	/** Show the workspace list (mobile back-to-team) — a real navigation. */
	openList: () => void
	/** Drop a starter prompt into this employee's composer. */
	askEmployee: (prompt: string) => void
	openCanvas: () => void
	triggerSummary: (wf: WorkflowConfig) => string
	persistWorkflows: (wfs: Record<string, WorkflowConfig>) => Promise<void>
	toggleWorkflow: (name: string) => Promise<void>
	toggleAgentStatus: (id: string, e?: MouseEvent) => void
	agentStatus: (id: string) => string
	refreshRuns: () => Promise<void>
	refreshThreads: () => Promise<void>
}
