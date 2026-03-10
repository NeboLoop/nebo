<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import {
		GitBranch,
		RefreshCw,
		Play,
		Power,
		Trash2,
		Clock,
		CheckCircle2,
		XCircle,
		Loader2,
		AlertCircle,
		ChevronRight,
		Activity,
		Coins,
		CalendarClock,
		Zap,
		SkipForward
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { WorkflowItem, WorkflowRun, ActivityResult } from '$lib/api/nebo';

	// State
	let workflows = $state<WorkflowItem[]>([]);
	let allRuns = $state<(WorkflowRun & { workflow_name: string })[]>([]);
	let isLoading = $state(true);
	let isLoadingRuns = $state(false);
	let togglingId = $state<string | null>(null);
	let deletingId = $state<string | null>(null);
	let runningId = $state<string | null>(null);

	// Detail modal
	let selectedRun = $state<WorkflowRun | null>(null);
	let selectedRunActivities = $state<ActivityResult[]>([]);
	let showRunDetail = $state(false);
	let loadingRunDetail = $state(false);

	// Selected workflow for run history
	let selectedWorkflow = $state<WorkflowItem | null>(null);
	let workflowRuns = $state<WorkflowRun[]>([]);
	let showWorkflowDetail = $state(false);

	// Polling interval for live runs
	let pollInterval: ReturnType<typeof setInterval> | null = null;

	const runningCount = $derived(allRuns.filter(r => r.status === 'running').length);

	onMount(async () => {
		await loadAll();
		startPolling();
	});

	onDestroy(() => {
		stopPolling();
	});

	function startPolling() {
		pollInterval = setInterval(async () => {
			if (runningCount > 0) {
				await loadRuns();
			}
		}, 3000);
	}

	function stopPolling() {
		if (pollInterval) {
			clearInterval(pollInterval);
			pollInterval = null;
		}
	}

	async function loadAll() {
		isLoading = true;
		try {
			const resp = await api.listWorkflows();
			workflows = resp.workflows || [];
			await loadRuns();
		} catch (e) {
			console.error('Failed to load workflows:', e);
		} finally {
			isLoading = false;
		}
	}

	async function loadRuns() {
		isLoadingRuns = true;
		try {
			// Aggregate runs from all workflows
			const runs: (WorkflowRun & { workflow_name: string })[] = [];
			await Promise.all(
				workflows.map(async (wf) => {
					try {
						const resp = await api.listWorkflowRuns(wf.id);
						(resp.runs || []).forEach(r => runs.push({ ...r, workflow_name: wf.name }));
					} catch {}
				})
			);
			// Sort by started_at desc
			runs.sort((a, b) => new Date(b.started_at).getTime() - new Date(a.started_at).getTime());
			allRuns = runs;
		} catch (e) {
			console.error('Failed to load runs:', e);
		} finally {
			isLoadingRuns = false;
		}
	}

	async function handleToggle(wf: WorkflowItem) {
		togglingId = wf.id;
		try {
			await api.toggleWorkflow(wf.id);
			await loadAll();
		} catch (e) {
			console.error('Failed to toggle workflow:', e);
		} finally {
			togglingId = null;
		}
	}

	async function handleDelete(wf: WorkflowItem) {
		if (!confirm(`Delete workflow "${wf.name}"? This cannot be undone.`)) return;
		deletingId = wf.id;
		try {
			await api.deleteWorkflow(wf.id);
			await loadAll();
		} catch (e) {
			console.error('Failed to delete workflow:', e);
		} finally {
			deletingId = null;
		}
	}

	async function handleRun(wf: WorkflowItem) {
		runningId = wf.id;
		try {
			await api.runWorkflow(wf.id);
			await loadRuns();
		} catch (e) {
			console.error('Failed to run workflow:', e);
		} finally {
			runningId = null;
		}
	}

	async function openWorkflowDetail(wf: WorkflowItem) {
		selectedWorkflow = wf;
		showWorkflowDetail = true;
		try {
			const resp = await api.listWorkflowRuns(wf.id);
			workflowRuns = (resp.runs || []).sort(
				(a, b) => new Date(b.started_at).getTime() - new Date(a.started_at).getTime()
			);
		} catch (e) {
			console.error('Failed to load workflow runs:', e);
		}
	}

	async function openRunDetail(run: WorkflowRun & { workflow_name?: string }) {
		selectedRun = run;
		showRunDetail = true;
		loadingRunDetail = true;
		try {
			const wf = workflows.find(w => w.id === run.workflow_id);
			if (wf) {
				const resp = await api.getWorkflowRun(run.workflow_id, run.id);
				selectedRunActivities = resp.activities || [];
			}
		} catch (e) {
			selectedRunActivities = [];
		} finally {
			loadingRunDetail = false;
		}
	}

	function statusIcon(status: string) {
		switch (status) {
			case 'running': return Loader2;
			case 'completed': return CheckCircle2;
			case 'failed': return XCircle;
			case 'aborted': return AlertCircle;
			default: return Clock;
		}
	}

	function statusClass(status: string) {
		switch (status) {
			case 'running': return 'text-info';
			case 'completed': return 'text-success';
			case 'failed': return 'text-error';
			case 'aborted': return 'text-warning';
			default: return 'text-base-content/40';
		}
	}

	function activityStatusIcon(status: string) {
		switch (status) {
			case 'completed': return CheckCircle2;
			case 'failed': return XCircle;
			case 'skipped': return SkipForward;
			default: return Clock;
		}
	}

	function formatDuration(start: string, end: string | null): string {
		if (!end) return '…';
		const ms = new Date(end).getTime() - new Date(start).getTime();
		if (ms < 1000) return `${ms}ms`;
		if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
		return `${Math.floor(ms / 60000)}m ${Math.floor((ms % 60000) / 1000)}s`;
	}

	function formatTime(ts: string): string {
		const d = new Date(ts);
		return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
	}

	function formatDate(ts: string): string {
		const d = new Date(ts);
		return d.toLocaleDateString([], { month: 'short', day: 'numeric' }) + ' ' +
			d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	}

	function parseDef(wf: WorkflowItem) {
		try { return JSON.parse(wf.definition); } catch { return null; }
	}

	// Live runs (running status)
	const liveRuns = $derived(allRuns.filter(r => r.status === 'running'));
	const recentRuns = $derived(allRuns.filter(r => r.status !== 'running').slice(0, 20));
</script>

<!-- Page header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Workflows</h2>
		<p class="text-sm text-base-content/60">Automated multi-step procedures running as subagents</p>
	</div>
	<button class="btn btn-ghost btn-sm" onclick={loadAll} disabled={isLoading}>
		<RefreshCw class="w-4 h-4 {isLoading ? 'animate-spin' : ''}" />
		Refresh
	</button>
</div>

{#if isLoading}
	<Card>
		<div class="py-12 text-center text-base-content/60">
			<span class="loading loading-spinner loading-md"></span>
			<p class="mt-2">Loading workflows...</p>
		</div>
	</Card>
{:else}

	<!-- Live runs banner -->
	{#if liveRuns.length > 0}
		<div class="mb-6 rounded-xl bg-info/5 ring-1 ring-info/20 p-4">
			<div class="flex items-center gap-2 mb-3">
				<span class="relative flex h-2 w-2">
					<span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-info opacity-75"></span>
					<span class="relative inline-flex rounded-full h-2 w-2 bg-info"></span>
				</span>
				<span class="text-sm font-semibold text-info">{liveRuns.length} running</span>
			</div>
			<div class="flex flex-col gap-2">
				{#each liveRuns as run}
					<button
						class="flex items-center gap-3 text-left w-full rounded-lg bg-base-100 px-3 py-2.5 ring-1 ring-base-content/5 hover:ring-info/30 transition-all"
						onclick={() => openRunDetail(run)}
					>
						<Loader2 class="w-4 h-4 text-info animate-spin shrink-0" />
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span class="text-sm font-medium text-base-content truncate">{run.workflow_name}</span>
								{#if run.current_activity}
									<span class="text-xs text-base-content/40">→ {run.current_activity}</span>
								{/if}
							</div>
							<div class="text-xs text-base-content/40">Started {formatTime(run.started_at)}</div>
						</div>
						<ChevronRight class="w-4 h-4 text-base-content/30 shrink-0" />
					</button>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Installed Workflows -->
	<div class="mb-8">
		<h3 class="text-sm font-semibold uppercase tracking-wider text-base-content/40 mb-4">Installed</h3>

		{#if workflows.length > 0}
			<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
				{#each workflows as wf}
					{@const def = parseDef(wf)}
					{@const isRunning = runningId === wf.id}
					{@const isToggling = togglingId === wf.id}
					{@const isDeleting = deletingId === wf.id}
					{@const activeRun = liveRuns.find(r => r.workflow_id === wf.id)}

					<div class="rounded-xl bg-base-100 p-4 shadow-sm ring-1 ring-base-content/5 transition-all hover:shadow-md {!wf.enabled ? 'opacity-60' : ''}">
						<!-- Header -->
						<div class="flex items-start gap-3 mb-3">
							<div class="w-9 h-9 rounded-lg {wf.enabled ? 'bg-primary/10' : 'bg-base-200'} flex items-center justify-center shrink-0 mt-0.5">
								{#if activeRun}
									<Loader2 class="w-4.5 h-4.5 text-info animate-spin" />
								{:else}
									<GitBranch class="w-4.5 h-4.5 {wf.enabled ? 'text-primary' : 'text-base-content/30'}" />
								{/if}
							</div>
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2 mb-0.5">
									<span class="font-display font-bold text-sm text-base-content truncate">{wf.name}</span>
									<span class="text-[10px] text-base-content/30 tabular-nums shrink-0">v{wf.version}</span>
								</div>
								{#if def?.activities}
									<div class="text-xs text-base-content/40">
										{def.activities.length} {def.activities.length === 1 ? 'activity' : 'activities'}
										{#if def.budget?.cost_estimate}
											· {def.budget.cost_estimate}/run
										{/if}
									</div>
								{/if}
							</div>
						</div>

						<!-- Triggers -->
						{#if def?.triggers?.length > 0}
							<div class="flex flex-wrap gap-1 mb-3">
								{#each def.triggers as trigger}
									<span class="badge badge-xs badge-ghost gap-1">
										{#if trigger.type === 'schedule'}
											<CalendarClock class="w-2.5 h-2.5" />
											{trigger.cron}
										{:else if trigger.type === 'event'}
											<Zap class="w-2.5 h-2.5" />
											{trigger.event}
										{:else}
											<Play class="w-2.5 h-2.5" />
											manual
										{/if}
									</span>
								{/each}
							</div>
						{/if}

						<!-- Active run progress -->
						{#if activeRun}
							<div class="mb-3 rounded-lg bg-info/5 px-3 py-2 ring-1 ring-info/15">
								<div class="flex items-center gap-2">
									<span class="text-xs text-info font-medium">Running</span>
									{#if activeRun.current_activity}
										<span class="text-xs text-base-content/50">→ {activeRun.current_activity}</span>
									{/if}
								</div>
							</div>
						{/if}

						<!-- Actions -->
						<div class="flex items-center gap-1 pt-1 border-t border-base-content/5">
							<button
								class="btn btn-xs btn-ghost gap-1 flex-1"
								onclick={() => openWorkflowDetail(wf)}
							>
								<Activity class="w-3 h-3" />
								History
							</button>
							<button
								class="btn btn-xs btn-ghost gap-1"
								onclick={() => handleRun(wf)}
								disabled={isRunning || !wf.enabled || !!activeRun}
								title="Run now"
							>
								{#if isRunning}
									<Loader2 class="w-3 h-3 animate-spin" />
								{:else}
									<Play class="w-3 h-3" />
								{/if}
							</button>
							<button
								class="btn btn-xs btn-ghost gap-1"
								onclick={() => handleToggle(wf)}
								disabled={isToggling}
								title={wf.enabled ? 'Disable' : 'Enable'}
							>
								{#if isToggling}
									<Loader2 class="w-3 h-3 animate-spin" />
								{:else}
									<Power class="w-3 h-3 {wf.enabled ? 'text-success' : 'text-base-content/30'}" />
								{/if}
							</button>
							<button
								class="btn btn-xs btn-ghost gap-1 hover:text-error"
								onclick={() => handleDelete(wf)}
								disabled={isDeleting}
								title="Delete"
							>
								{#if isDeleting}
									<Loader2 class="w-3 h-3 animate-spin" />
								{:else}
									<Trash2 class="w-3 h-3" />
								{/if}
							</button>
						</div>
					</div>
				{/each}
			</div>
		{:else}
			<Card>
				<div class="py-12 text-center text-base-content/60">
					<GitBranch class="w-12 h-12 mx-auto mb-4 opacity-20" />
					<p class="font-medium mb-2">No workflows installed</p>
					<p class="text-sm">Install a workflow using a WORK-XXXX-XXXX code.</p>
				</div>
			</Card>
		{/if}
	</div>

	<!-- Recent Runs -->
	<div>
		<div class="flex items-center justify-between mb-4">
			<h3 class="text-sm font-semibold uppercase tracking-wider text-base-content/40">Recent Runs</h3>
			{#if isLoadingRuns}
				<span class="loading loading-spinner loading-xs text-base-content/30"></span>
			{/if}
		</div>

		{#if recentRuns.length > 0}
			<div class="rounded-xl bg-base-100 ring-1 ring-base-content/5 overflow-hidden">
				{#each recentRuns as run, i}
					{@const Icon = statusIcon(run.status)}
					<button
						class="w-full flex items-center gap-3 px-4 py-3 text-left hover:bg-base-200/50 transition-colors {i > 0 ? 'border-t border-base-content/5' : ''}"
						onclick={() => openRunDetail(run)}
					>
						<Icon class="w-4 h-4 shrink-0 {statusClass(run.status)} {run.status === 'running' ? 'animate-spin' : ''}" />
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span class="text-sm font-medium text-base-content truncate">{run.workflow_name}</span>
								<span class="badge badge-xs {run.status === 'completed' ? 'badge-success' : run.status === 'failed' ? 'badge-error' : run.status === 'running' ? 'badge-info' : 'badge-warning'}">{run.status}</span>
							</div>
							<div class="flex items-center gap-3 text-xs text-base-content/40 mt-0.5">
								<span class="flex items-center gap-1">
									<Clock class="w-3 h-3" />
									{formatDate(run.started_at)}
								</span>
								<span>{formatDuration(run.started_at, run.completed_at)}</span>
								{#if run.total_tokens_used > 0}
									<span class="flex items-center gap-1">
										<Coins class="w-3 h-3" />
										{run.total_tokens_used.toLocaleString()}
									</span>
								{/if}
								<span class="capitalize">{run.trigger_type}</span>
							</div>
						</div>
						<ChevronRight class="w-4 h-4 text-base-content/20 shrink-0" />
					</button>
				{/each}
			</div>
		{:else if !isLoadingRuns}
			<Card>
				<div class="py-8 text-center text-base-content/60">
					<Activity class="w-10 h-10 mx-auto mb-3 opacity-20" />
					<p class="font-medium mb-1">No runs yet</p>
					<p class="text-sm">Run a workflow manually or wait for a scheduled trigger.</p>
				</div>
			</Card>
		{/if}
	</div>

{/if}

<!-- Workflow run history modal -->
{#if showWorkflowDetail && selectedWorkflow}
	<div class="fixed inset-0 z-50 flex items-center justify-center p-4">
		<button class="absolute inset-0 bg-black/50" aria-label="Close" onclick={() => { showWorkflowDetail = false; }}></button>
		<div class="relative w-full max-w-lg bg-base-100 rounded-2xl shadow-2xl overflow-hidden max-h-[80vh] flex flex-col">
			<div class="flex items-center justify-between px-6 py-4 border-b border-base-content/5">
				<div>
					<h3 class="font-display font-bold text-base-content">{selectedWorkflow.name}</h3>
					<p class="text-xs text-base-content/40 mt-0.5">Run history</p>
				</div>
				<button class="btn btn-ghost btn-sm btn-square" onclick={() => { showWorkflowDetail = false; }}>✕</button>
			</div>
			<div class="overflow-y-auto flex-1 p-4">
				{#if workflowRuns.length > 0}
					<div class="flex flex-col gap-2">
						{#each workflowRuns as run}
							{@const Icon = statusIcon(run.status)}
							<button
								class="flex items-center gap-3 text-left rounded-xl bg-base-200/40 px-4 py-3 hover:bg-base-200 transition-colors ring-1 ring-base-content/5"
								onclick={() => { showWorkflowDetail = false; openRunDetail({ ...run, workflow_name: selectedWorkflow!.name }); }}
							>
								<Icon class="w-4 h-4 shrink-0 {statusClass(run.status)} {run.status === 'running' ? 'animate-spin' : ''}" />
								<div class="flex-1 min-w-0">
									<div class="flex items-center gap-2 mb-0.5">
										<span class="badge badge-xs {run.status === 'completed' ? 'badge-success' : run.status === 'failed' ? 'badge-error' : run.status === 'running' ? 'badge-info' : 'badge-warning'}">{run.status}</span>
										<span class="text-xs text-base-content/40 capitalize">{run.trigger_type}</span>
									</div>
									<div class="flex items-center gap-3 text-xs text-base-content/40">
										<span>{formatDate(run.started_at)}</span>
										<span>{formatDuration(run.started_at, run.completed_at)}</span>
										{#if run.total_tokens_used > 0}
											<span>{run.total_tokens_used.toLocaleString()} tokens</span>
										{/if}
									</div>
								</div>
								<ChevronRight class="w-4 h-4 text-base-content/20 shrink-0" />
							</button>
						{/each}
					</div>
				{:else}
					<div class="py-8 text-center text-base-content/40">
						<Activity class="w-8 h-8 mx-auto mb-2 opacity-30" />
						<p class="text-sm">No runs yet</p>
					</div>
				{/if}
			</div>
		</div>
	</div>
{/if}

<!-- Run detail modal -->
{#if showRunDetail && selectedRun}
	{@const RunIcon = statusIcon(selectedRun.status)}
	<div class="fixed inset-0 z-50 flex items-center justify-center p-4">
		<button class="absolute inset-0 bg-black/50" aria-label="Close" onclick={() => { showRunDetail = false; selectedRun = null; }}></button>
		<div class="relative w-full max-w-lg bg-base-100 rounded-2xl shadow-2xl overflow-hidden max-h-[85vh] flex flex-col">
			<!-- Header -->
			<div class="flex items-center justify-between px-6 py-4 border-b border-base-content/5">
				<div class="flex items-center gap-3">
					<RunIcon class="w-5 h-5 {statusClass(selectedRun.status)} {selectedRun.status === 'running' ? 'animate-spin' : ''}" />
					<div>
						<h3 class="font-display font-bold text-base-content text-sm">Run {selectedRun.id.slice(0, 8)}</h3>
						<p class="text-xs text-base-content/40">{formatDate(selectedRun.started_at)}</p>
					</div>
				</div>
				<button class="btn btn-ghost btn-sm btn-square" onclick={() => { showRunDetail = false; selectedRun = null; }}>✕</button>
			</div>

			<div class="overflow-y-auto flex-1 p-5 flex flex-col gap-4">
				<!-- Stats row -->
				<div class="grid grid-cols-3 gap-2">
					<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
						<div class="text-[10px] uppercase tracking-wide text-base-content/40 mb-1">Duration</div>
						<div class="text-sm font-bold text-base-content tabular-nums">{formatDuration(selectedRun.started_at, selectedRun.completed_at)}</div>
					</div>
					<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
						<div class="text-[10px] uppercase tracking-wide text-base-content/40 mb-1">Tokens</div>
						<div class="text-sm font-bold text-base-content tabular-nums">{selectedRun.total_tokens_used.toLocaleString()}</div>
					</div>
					<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
						<div class="text-[10px] uppercase tracking-wide text-base-content/40 mb-1">Trigger</div>
						<div class="text-sm font-bold text-base-content capitalize">{selectedRun.trigger_type}</div>
					</div>
				</div>

				<!-- Error -->
				{#if selectedRun.error}
					<div class="rounded-xl bg-error/5 ring-1 ring-error/20 px-4 py-3">
						<div class="flex items-center gap-2 mb-1">
							<XCircle class="w-3.5 h-3.5 text-error" />
							<span class="text-xs font-semibold text-error">Failed{selectedRun.error_activity ? ` at "${selectedRun.error_activity}"` : ''}</span>
						</div>
						<p class="text-xs text-base-content/60 font-mono leading-relaxed">{selectedRun.error}</p>
					</div>
				{/if}

				<!-- Activities -->
				<div>
					<h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/40 mb-3">Activities</h4>
					{#if loadingRunDetail}
						<div class="py-4 text-center">
							<span class="loading loading-spinner loading-sm text-base-content/30"></span>
						</div>
					{:else if selectedRunActivities.length > 0}
						<div class="flex flex-col gap-1.5">
							{#each selectedRunActivities as act}
								{@const AIcon = activityStatusIcon(act.status)}
								<div class="flex items-center gap-3 rounded-lg bg-base-200/40 px-3 py-2.5">
									<AIcon class="w-3.5 h-3.5 shrink-0 {act.status === 'completed' ? 'text-success' : act.status === 'failed' ? 'text-error' : 'text-base-content/30'}" />
									<div class="flex-1 min-w-0">
										<div class="text-xs font-medium text-base-content">{act.activity_id}</div>
										{#if act.error}
											<div class="text-xs text-error/70 truncate mt-0.5">{act.error}</div>
										{/if}
									</div>
									<div class="text-right shrink-0">
										<div class="text-xs text-base-content/40 tabular-nums">{formatDuration(act.started_at, act.completed_at)}</div>
										{#if act.tokens_used > 0}
											<div class="text-[10px] text-base-content/30 tabular-nums">{act.tokens_used.toLocaleString()} tok</div>
										{/if}
									</div>
								</div>
							{/each}
						</div>
					{:else}
						<div class="py-4 text-center text-xs text-base-content/40">No activity data available</div>
					{/if}
				</div>
			</div>
		</div>
	</div>
{/if}
