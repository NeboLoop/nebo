<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import SearchInput from '$lib/components/ui/SearchInput.svelte';
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
		SkipForward,
		StopCircle,
		Hash,
		Download,
		Store
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import { listStoreWorkflows, neboLoopOAuthStartWithJanus } from '$lib/api';
	import type { StoreWorkflowItem } from '$lib/api/neboComponents';
	import type { WorkflowItem, WorkflowRun, ActivityResult } from '$lib/api/nebo';

	// ─── State ───────────────────────────────────────────────────────────
	let workflows = $state<WorkflowItem[]>([]);
	let isLoading = $state(true);
	let searchQuery = $state('');
	let selectedId = $state<string | null>(null);

	// Selected workflow runs
	let runs = $state<WorkflowRun[]>([]);
	let isLoadingRuns = $state(false);

	// Action states
	let togglingId = $state<string | null>(null);
	let deletingId = $state<string | null>(null);
	let runningId = $state<string | null>(null);
	let cancellingRunId = $state<string | null>(null);

	// Marketplace
	let marketplaceWorkflows = $state<StoreWorkflowItem[]>([]);
	let marketplaceLoading = $state(false);
	let neboLoopConnected = $state(false);

	// Run detail modal
	let selectedRun = $state<WorkflowRun | null>(null);
	let selectedRunActivities = $state<ActivityResult[]>([]);
	let showRunDetail = $state(false);
	let loadingRunDetail = $state(false);

	// Polling
	let pollInterval: ReturnType<typeof setInterval> | null = null;

	// ─── Derived ─────────────────────────────────────────────────────────
	const filteredWorkflows = $derived(
		searchQuery
			? workflows.filter(wf =>
				wf.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				(wf.code && wf.code.toLowerCase().includes(searchQuery.toLowerCase()))
			)
			: workflows
	);

	const selected = $derived(workflows.find(wf => wf.id === selectedId) ?? null);

	const filteredMarketplace = $derived(
		searchQuery
			? marketplaceWorkflows.filter(wf =>
				wf.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				wf.description.toLowerCase().includes(searchQuery.toLowerCase())
			)
			: marketplaceWorkflows
	);

	const liveRuns = $derived(runs.filter(r => r.status === 'running'));
	const failedRuns = $derived(runs.filter(r => r.status === 'failed').slice(0, 5));
	const recentRuns = $derived(
		runs.sort((a, b) => new Date(b.started_at).getTime() - new Date(a.started_at).getTime()).slice(0, 20)
	);

	// ─── Lifecycle ───────────────────────────────────────────────────────
	onMount(async () => {
		await loadWorkflows();
		startPolling();
		loadMarketplace();
	});

	onDestroy(() => stopPolling());

	function startPolling() {
		pollInterval = setInterval(async () => {
			if (selectedId && liveRuns.length > 0) {
				await loadRuns(selectedId);
			}
		}, 3000);
	}

	function stopPolling() {
		if (pollInterval) {
			clearInterval(pollInterval);
			pollInterval = null;
		}
	}

	// ─── Data Loading ────────────────────────────────────────────────────
	async function loadWorkflows() {
		isLoading = true;
		try {
			const resp = await api.listWorkflows();
			workflows = resp.workflows || [];
			// Auto-select first workflow
			if (workflows.length > 0 && !selectedId) {
				selectWorkflow(workflows[0].id);
			} else if (selectedId) {
				// Refresh runs for currently selected
				await loadRuns(selectedId);
			}
		} catch (e) {
			console.error('Failed to load workflows:', e);
		} finally {
			isLoading = false;
		}
	}

	async function loadMarketplace() {
		try {
			const status = await api.neboLoopStatus();
			neboLoopConnected = status.connected;
			if (!status.connected) return;
			marketplaceLoading = true;
			const resp = await listStoreWorkflows();
			marketplaceWorkflows = resp.workflows || [];
		} catch {
			// NeboLoop not available
		} finally {
			marketplaceLoading = false;
		}
	}

	async function loadRuns(workflowId: string) {
		isLoadingRuns = true;
		try {
			const resp = await api.listWorkflowRuns(workflowId);
			runs = (resp.runs || []).sort(
				(a, b) => new Date(b.started_at).getTime() - new Date(a.started_at).getTime()
			);
		} catch (e) {
			console.error('Failed to load runs:', e);
		} finally {
			isLoadingRuns = false;
		}
	}

	async function selectWorkflow(id: string) {
		selectedId = id;
		runs = [];
		await loadRuns(id);
	}

	// ─── Actions ─────────────────────────────────────────────────────────
	async function handleToggle(wf: WorkflowItem) {
		togglingId = wf.id;
		try {
			await api.toggleWorkflow(wf.id);
			await loadWorkflows();
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
			// Select next workflow or clear
			const remaining = workflows.filter(w => w.id !== wf.id);
			if (remaining.length > 0) {
				selectedId = remaining[0].id;
			} else {
				selectedId = null;
				runs = [];
			}
			await loadWorkflows();
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
			await loadRuns(wf.id);
		} catch (e) {
			console.error('Failed to run workflow:', e);
		} finally {
			runningId = null;
		}
	}

	async function handleCancelRun(run: WorkflowRun) {
		cancellingRunId = run.id;
		try {
			await api.cancelWorkflowRun(run.workflow_id, run.id);
			if (selectedId) await loadRuns(selectedId);
		} catch (e) {
			console.error('Failed to cancel run:', e);
		} finally {
			cancellingRunId = null;
		}
	}

	async function openRunDetail(run: WorkflowRun) {
		selectedRun = run;
		showRunDetail = true;
		loadingRunDetail = true;
		try {
			const resp = await api.getWorkflowRun(run.workflow_id, run.id);
			selectedRunActivities = resp.activities || [];
		} catch {
			selectedRunActivities = [];
		} finally {
			loadingRunDetail = false;
		}
	}

	// ─── Helpers ─────────────────────────────────────────────────────────
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
			default: return 'text-base-content/70';
		}
	}

	function statusBadgeClass(status: string) {
		switch (status) {
			case 'completed': return 'badge-success';
			case 'failed': return 'badge-error';
			case 'running': return 'badge-info';
			case 'aborted': return 'badge-warning';
			default: return 'badge-ghost';
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
		if (!end) return '...';
		const ms = new Date(end).getTime() - new Date(start).getTime();
		if (ms < 1000) return `${ms}ms`;
		if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
		return `${Math.floor(ms / 60000)}m ${Math.floor((ms % 60000) / 1000)}s`;
	}

	function formatTime(ts: string): string {
		return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
	}

	function formatDate(ts: string): string {
		const d = new Date(ts);
		return d.toLocaleDateString([], { month: 'short', day: 'numeric' }) + ' ' +
			d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	}

	function parseDef(wf: WorkflowItem) {
		try { return JSON.parse(wf.definition); } catch { return null; }
	}
</script>

<!-- Page header -->
<div class="flex items-center justify-between mb-4" style="height: 52px;">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-0.5">Workflows</h2>
		<p class="text-sm text-base-content/70">Automated multi-step procedures</p>
	</div>
	<button class="btn btn-ghost btn-sm" onclick={() => loadWorkflows()} disabled={isLoading} aria-label="Refresh workflows">
		<RefreshCw class="w-4 h-4 {isLoading ? 'animate-spin' : ''}" />
		Refresh
	</button>
</div>

{#if isLoading && workflows.length === 0}
	<div class="flex-1 flex items-center justify-center">
		<div class="text-center text-base-content/70">
			<span class="loading loading-spinner loading-md"></span>
			<p class="mt-2">Loading workflows...</p>
		</div>
	</div>
{:else}
	<div class="workflow-page">
		<!-- ═══ Left Sidebar ═══ -->
		<aside class="sidebar-container scrollbar-thin">
			<div class="p-3">
				<SearchInput
					bind:value={searchQuery}
					placeholder="Search workflows..."
					size="sm"
				/>
			</div>

			<!-- Installed section -->
			<div class="sidebar-section-label">Installed</div>
			<nav class="sidebar-nav">
				{#if filteredWorkflows.length > 0}
					{#each filteredWorkflows as wf (wf.id)}
						{@const activeRun = selectedId === wf.id ? liveRuns.find(r => r.workflow_id === wf.id) : null}
						<button
							class="sidebar-item"
							class:sidebar-item-active={selectedId === wf.id}
							onclick={() => selectWorkflow(wf.id)}
						>
							{#if activeRun}
								<Loader2 class="sidebar-icon text-info animate-spin" />
							{:else}
								<GitBranch class="sidebar-icon {wf.enabled ? '' : 'opacity-30'}" />
							{/if}
							<span class="sidebar-label">{wf.name}</span>
							{#if !wf.enabled}
								<span class="badge badge-xs badge-ghost">off</span>
							{/if}
						</button>
					{/each}
				{:else if searchQuery}
					<div class="px-4 py-6 text-center text-sm text-base-content/70">
						No matches
					</div>
				{:else}
					<div class="px-4 py-6 text-center text-sm text-base-content/70">
						No workflows installed
					</div>
				{/if}
			</nav>

			<!-- Marketplace -->
			<div class="sidebar-section-label">Marketplace</div>
			{#if marketplaceLoading}
				<div class="px-4 py-4 text-center">
					<span class="loading loading-spinner loading-xs text-base-content/70"></span>
				</div>
			{:else if !neboLoopConnected}
				<div class="px-4 py-4 text-xs text-base-content/70 text-center">
					Connect to NeboLoop to browse
				</div>
			{:else if filteredMarketplace.length > 0}
				<nav class="sidebar-nav">
					{#each filteredMarketplace as mw (mw.id)}
						<div class="sidebar-item">
							<Store class="sidebar-icon opacity-50" />
							<div class="flex-1 min-w-0">
								<span class="sidebar-label block truncate">{mw.name}</span>
								<span class="text-[10px] text-base-content/70 flex items-center gap-1">
									v{mw.version}
									{#if mw.installCount > 0}
										<Download class="w-2.5 h-2.5 inline" />
										{mw.installCount}
									{/if}
								</span>
							</div>
							{#if mw.isInstalled}
								<span class="badge badge-xs badge-success">installed</span>
							{/if}
						</div>
					{/each}
				</nav>
			{:else if searchQuery}
				<div class="px-4 py-4 text-xs text-base-content/70 text-center">
					No matches
				</div>
			{:else}
				<div class="px-4 py-4 text-xs text-base-content/70 text-center">
					No workflows available
				</div>
			{/if}
		</aside>

		<!-- ═══ Right Detail Panel ═══ -->
		<main class="flex-1 overflow-y-auto scrollbar-thin p-6">
			{#if selected}
				{@const def = parseDef(selected)}
				{@const isRunning = runningId === selected.id}
				{@const isToggling = togglingId === selected.id}
				{@const isDeleting = deletingId === selected.id}

				{#snippet detailHeader()}
					<div class="flex items-start justify-between gap-4 mb-6">
						<div class="min-w-0">
							<div class="flex items-center gap-3 mb-1">
								<h3 class="font-display text-lg font-bold text-base-content truncate">{selected.name}</h3>
								<span class="text-xs text-base-content/70 tabular-nums shrink-0">v{selected.version}</span>
								<span class="badge badge-sm {selected.enabled ? 'badge-success' : 'badge-ghost'}">
									{selected.enabled ? 'Enabled' : 'Disabled'}
								</span>
							</div>
							{#if selected.code}
								<div class="flex items-center gap-1.5 text-xs text-base-content/70">
									<Hash class="w-3 h-3" />
									<span class="font-mono">{selected.code}</span>
								</div>
							{/if}
						</div>
						<div class="flex items-center gap-1.5 shrink-0">
							<button
								class="btn btn-sm btn-primary gap-1.5"
								onclick={() => handleRun(selected)}
								disabled={isRunning || !selected.enabled || liveRuns.some(r => r.workflow_id === selected.id)}
								aria-label="Run workflow"
							>
								{#if isRunning}
									<Loader2 class="w-3.5 h-3.5 animate-spin" />
								{:else}
									<Play class="w-3.5 h-3.5" />
								{/if}
								Run
							</button>
							<button
								class="btn btn-sm btn-ghost gap-1"
								onclick={() => handleToggle(selected)}
								disabled={isToggling}
								aria-label={selected.enabled ? 'Disable workflow' : 'Enable workflow'}
							>
								{#if isToggling}
									<Loader2 class="w-3.5 h-3.5 animate-spin" />
								{:else}
									<Power class="w-3.5 h-3.5 {selected.enabled ? 'text-success' : 'text-base-content/70'}" />
								{/if}
							</button>
							<button
								class="btn btn-sm btn-ghost hover:text-error gap-1"
								onclick={() => handleDelete(selected)}
								disabled={isDeleting}
								aria-label="Delete workflow"
							>
								{#if isDeleting}
									<Loader2 class="w-3.5 h-3.5 animate-spin" />
								{:else}
									<Trash2 class="w-3.5 h-3.5" />
								{/if}
							</button>
						</div>
					</div>
				{/snippet}

				{#snippet overviewSection()}
					<!-- Stats row -->
					<div class="grid grid-cols-3 gap-2 mb-5">
						<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
							<div class="text-[10px] uppercase tracking-wide text-base-content/70 mb-0.5">Activities</div>
							<div class="text-sm font-bold text-base-content tabular-nums">{def?.activities?.length ?? 0}</div>
						</div>
						<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
							<div class="text-[10px] uppercase tracking-wide text-base-content/70 mb-0.5">Total Runs</div>
							<div class="text-sm font-bold text-base-content tabular-nums">{runs.length}</div>
						</div>
						<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
							<div class="text-[10px] uppercase tracking-wide text-base-content/70 mb-0.5">Budget</div>
							<div class="text-sm font-bold text-base-content">{def?.budget?.cost_estimate ?? '--'}</div>
						</div>
					</div>

					<!-- Activities list -->
					{#if def?.activities?.length > 0}
						<div class="mb-5">
							<h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/70 mb-2">Activities</h4>
							<div class="flex flex-col gap-1">
								{#each def.activities as act, i}
									<div class="flex items-center gap-2.5 rounded-lg bg-base-200/30 px-3 py-2">
										<span class="w-5 h-5 rounded-full bg-base-300 flex items-center justify-center text-[10px] font-bold text-base-content/70 shrink-0">{i + 1}</span>
										<span class="text-sm text-base-content">{act.intent || act.id || act.name || `Step ${i + 1}`}</span>
									</div>
								{/each}
							</div>
						</div>
					{/if}

					<!-- Inputs list -->
					{#if def?.inputs && Object.keys(def.inputs).length > 0}
						<div class="mb-5">
							<h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/70 mb-2">Inputs</h4>
							<div class="flex flex-col gap-1">
								{#each Object.entries(def.inputs) as [key, input]}
									{@const inp = input as Record<string, unknown>}
									<div class="flex items-center justify-between rounded-lg bg-base-200/30 px-3 py-2">
										<span class="text-sm font-medium text-base-content">{key}</span>
										<span class="text-xs text-base-content/70">
											{inp.type ?? 'string'}
											{#if inp.default != null}
												<span class="ml-1 text-base-content/25">= {inp.default}</span>
											{/if}
										</span>
									</div>
								{/each}
							</div>
						</div>
					{/if}

					<!-- Triggers -->
					{#if def?.triggers?.length > 0}
						<div class="mb-5">
							<h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/70 mb-2">Triggers</h4>
							<div class="flex flex-wrap gap-1.5">
								{#each def.triggers as trigger}
									<span class="badge badge-sm badge-ghost gap-1">
										{#if trigger.type === 'schedule'}
											<CalendarClock class="w-3 h-3" />
											{trigger.cron}
										{:else if trigger.type === 'event'}
											<Zap class="w-3 h-3" />
											{trigger.event}
										{:else}
											<Play class="w-3 h-3" />
											manual
										{/if}
									</span>
								{/each}
							</div>
						</div>
					{/if}
				{/snippet}

				{#snippet liveRunsBanner()}
					{#if liveRuns.length > 0}
						<div class="mb-5 rounded-xl bg-info/5 ring-1 ring-info/20 p-4">
							<div class="flex items-center gap-2 mb-2">
								<span class="relative flex h-2 w-2">
									<span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-info opacity-75"></span>
									<span class="relative inline-flex rounded-full h-2 w-2 bg-info"></span>
								</span>
								<span class="text-sm font-semibold text-info">{liveRuns.length} running</span>
							</div>
							<div class="flex flex-col gap-1.5">
								{#each liveRuns as run}
									<div class="flex items-center gap-3 rounded-lg bg-base-100 px-3 py-2 ring-1 ring-base-content/5">
										<Loader2 class="w-4 h-4 text-info animate-spin shrink-0" />
										<div class="flex-1 min-w-0">
											{#if run.current_activity}
												<span class="text-xs text-base-content/70">{run.current_activity}</span>
											{:else}
												<span class="text-xs text-base-content/70">Starting...</span>
											{/if}
											<div class="text-[10px] text-base-content/70">Started {formatTime(run.started_at)}</div>
										</div>
										<button
											class="btn btn-xs btn-ghost text-warning gap-1"
											onclick={() => handleCancelRun(run)}
											disabled={cancellingRunId === run.id}
											aria-label="Cancel run"
										>
											{#if cancellingRunId === run.id}
												<Loader2 class="w-3 h-3 animate-spin" />
											{:else}
												<StopCircle class="w-3 h-3" />
											{/if}
											Cancel
										</button>
									</div>
								{/each}
							</div>
						</div>
					{/if}
				{/snippet}

				{#snippet recentRunsSection()}
					<div class="mb-5">
						<div class="flex items-center justify-between mb-2">
							<h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/70">Recent Runs</h4>
							{#if isLoadingRuns}
								<span class="loading loading-spinner loading-xs text-base-content/70"></span>
							{/if}
						</div>

						{#if recentRuns.length > 0}
							<div class="rounded-xl bg-base-100 ring-1 ring-base-content/5 overflow-hidden">
								{#each recentRuns as run, i}
									{@const Icon = statusIcon(run.status)}
									<button
										class="w-full flex items-center gap-3 px-4 py-2.5 text-left hover:bg-base-200/50 transition-colors cursor-pointer {i > 0 ? 'border-t border-base-content/5' : ''}"
										onclick={() => openRunDetail(run)}
									>
										<Icon class="w-4 h-4 shrink-0 {statusClass(run.status)} {run.status === 'running' ? 'animate-spin' : ''}" />
										<div class="flex-1 min-w-0">
											<div class="flex items-center gap-2">
												<span class="badge badge-xs {statusBadgeClass(run.status)}">{run.status}</span>
												<span class="text-xs text-base-content/70 capitalize">{run.trigger_type}</span>
											</div>
											<div class="flex items-center gap-3 text-xs text-base-content/70 mt-0.5">
												<span class="flex items-center gap-1">
													<Clock class="w-3 h-3" />
													{formatDate(run.started_at)}
												</span>
												<span class="tabular-nums">{formatDuration(run.started_at, run.completed_at)}</span>
												{#if run.total_tokens_used > 0}
													<span class="flex items-center gap-1 tabular-nums">
														<Coins class="w-3 h-3" />
														{run.total_tokens_used.toLocaleString()}
													</span>
												{/if}
											</div>
										</div>
										<ChevronRight class="w-4 h-4 text-base-content/20 shrink-0" />
									</button>
								{/each}
							</div>
						{:else if !isLoadingRuns}
							<div class="rounded-xl bg-base-200/30 py-8 text-center">
								<Activity class="w-8 h-8 mx-auto mb-2 text-base-content/15" />
								<p class="text-sm text-base-content/70">No runs yet</p>
								<p class="text-xs text-base-content/25 mt-0.5">Run this workflow manually or wait for a trigger.</p>
							</div>
						{/if}
					</div>
				{/snippet}

				{#snippet errorsSection()}
					{#if failedRuns.length > 0}
						<div>
							<h4 class="text-xs font-semibold uppercase tracking-wider text-error/60 mb-2">Recent Errors</h4>
							<div class="flex flex-col gap-1.5">
								{#each failedRuns as run}
									<button
										class="flex items-start gap-2.5 rounded-xl bg-error/5 ring-1 ring-error/10 px-3 py-2.5 text-left hover:ring-error/25 transition-all cursor-pointer w-full"
										onclick={() => openRunDetail(run)}
									>
										<XCircle class="w-3.5 h-3.5 text-error shrink-0 mt-0.5" />
										<div class="flex-1 min-w-0">
											<div class="flex items-center gap-2 mb-0.5">
												<span class="text-xs font-medium text-error">
													{run.error_activity ? `Failed at "${run.error_activity}"` : 'Failed'}
												</span>
												<span class="text-[10px] text-base-content/70">{formatDate(run.started_at)}</span>
											</div>
											{#if run.error}
												<p class="text-xs text-base-content/70 font-mono truncate">{run.error}</p>
											{/if}
										</div>
									</button>
								{/each}
							</div>
						</div>
					{/if}
				{/snippet}

				<!-- Render all sections -->
				{@render detailHeader()}
				{@render overviewSection()}
				{@render liveRunsBanner()}
				{@render recentRunsSection()}
				{@render errorsSection()}

			{:else}
				<!-- Empty state: no workflow selected -->
				<div class="flex-1 flex items-center justify-center h-full">
					<div class="text-center">
						<GitBranch class="w-12 h-12 mx-auto mb-4 text-base-content/10" />
						{#if workflows.length === 0}
							<p class="font-medium text-base-content/70 mb-1">No workflows installed</p>
							<p class="text-sm text-base-content/70">Install a workflow using a WORK-XXXX-XXXX code.</p>
						{:else}
							<p class="font-medium text-base-content/70 mb-1">Select a workflow</p>
							<p class="text-sm text-base-content/70">Choose a workflow from the sidebar to view details.</p>
						{/if}
					</div>
				</div>
			{/if}
		</main>
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
						<p class="text-xs text-base-content/70">{formatDate(selectedRun.started_at)}</p>
					</div>
				</div>
				<button class="btn btn-ghost btn-sm btn-square" onclick={() => { showRunDetail = false; selectedRun = null; }} aria-label="Close modal">
					<XCircle class="w-4 h-4" />
				</button>
			</div>

			<div class="overflow-y-auto flex-1 p-5 flex flex-col gap-4">
				<!-- Stats row -->
				<div class="grid grid-cols-3 gap-2">
					<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
						<div class="text-[10px] uppercase tracking-wide text-base-content/70 mb-1">Duration</div>
						<div class="text-sm font-bold text-base-content tabular-nums">{formatDuration(selectedRun.started_at, selectedRun.completed_at)}</div>
					</div>
					<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
						<div class="text-[10px] uppercase tracking-wide text-base-content/70 mb-1">Tokens</div>
						<div class="text-sm font-bold text-base-content tabular-nums">{selectedRun.total_tokens_used.toLocaleString()}</div>
					</div>
					<div class="rounded-xl bg-base-200/50 px-3 py-2.5 text-center">
						<div class="text-[10px] uppercase tracking-wide text-base-content/70 mb-1">Trigger</div>
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
						<p class="text-xs text-base-content/70 font-mono leading-relaxed">{selectedRun.error}</p>
					</div>
				{/if}

				<!-- Activities -->
				<div>
					<h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/70 mb-3">Activities</h4>
					{#if loadingRunDetail}
						<div class="py-4 text-center">
							<span class="loading loading-spinner loading-sm text-base-content/70"></span>
						</div>
					{:else if selectedRunActivities.length > 0}
						<div class="flex flex-col gap-1.5">
							{#each selectedRunActivities as act}
								{@const AIcon = activityStatusIcon(act.status)}
								<div class="flex items-center gap-3 rounded-lg bg-base-200/40 px-3 py-2.5">
									<AIcon class="w-3.5 h-3.5 shrink-0 {act.status === 'completed' ? 'text-success' : act.status === 'failed' ? 'text-error' : 'text-base-content/70'}" />
									<div class="flex-1 min-w-0">
										<div class="text-xs font-medium text-base-content">{act.activity_id}</div>
										{#if act.error}
											<div class="text-xs text-error/70 truncate mt-0.5">{act.error}</div>
										{/if}
									</div>
									<div class="text-right shrink-0">
										<div class="text-xs text-base-content/70 tabular-nums">{formatDuration(act.started_at, act.completed_at)}</div>
										{#if act.tokens_used > 0}
											<div class="text-[10px] text-base-content/70 tabular-nums">{act.tokens_used.toLocaleString()} tok</div>
										{/if}
									</div>
								</div>
							{/each}
						</div>
					{:else}
						<div class="py-4 text-center text-xs text-base-content/70">No activity data available</div>
					{/if}
				</div>
			</div>
		</div>
	</div>
{/if}
