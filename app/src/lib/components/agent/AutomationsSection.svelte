<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { getEntityConfig, updateEntityConfig, getRoleWorkflows, createRoleWorkflow, toggleRoleWorkflow, deleteRoleWorkflow } from '$lib/api/nebo';
	import type { RoleWorkflowEntry } from '$lib/api/neboComponents';
	import AutomationEditor from './AutomationEditor.svelte';
	import RichInput from '$lib/components/ui/RichInput.svelte';
	import { Plus, Pencil, Trash2, Store, Copy, MoreHorizontal, X } from 'lucide-svelte';
	import { fly } from 'svelte/transition';
	import { getWebSocketClient } from '$lib/websocket/client';

	let {
		entityType,
		entityId,
		roleId,
		readonly = false,
	}: {
		entityType: string;
		entityId: string;
		roleId?: string;
		readonly?: boolean;
	} = $props();

	// Operating mode
	let mode = $state<'heartbeat' | 'automations'>('heartbeat');
	let modeInitialized = false;

	// Heartbeat state
	let loading = $state(true);
	let saving = $state(false);
	let heartbeatEnabled = $state(false);
	let heartbeatInterval = $state(60);
	let heartbeatContent = $state('');
	let heartbeatWindow = $state<[string, string] | null>(null);

	// Automations enabled state
	let automationsEnabled = $state(false);

	// Workflow bindings state
	let workflows = $state<RoleWorkflowEntry[]>([]);
	let showEditor = $state(false);
	let editingWorkflow: RoleWorkflowEntry | null = $state(null);
	let confirmDelete: string | null = $state(null);
	let toggling: string | null = $state(null);
	let overflowMenu: string | null = $state(null);
	let previewWorkflow: RoleWorkflowEntry | null = $state(null);

	const intervalOptions = [
		{ value: 1, label: '1 minute' },
		{ value: 5, label: '5 minutes' },
		{ value: 10, label: '10 minutes' },
		{ value: 15, label: '15 minutes' },
		{ value: 30, label: '30 minutes' },
		{ value: 60, label: '1 hour' },
		{ value: 120, label: '2 hours' },
		{ value: 240, label: '4 hours' },
		{ value: 480, label: '8 hours' },
		{ value: 1440, label: '24 hours' },
	];

	const triggerIcons: Record<string, string> = {
		schedule: '📅',
		heartbeat: '⏱',
		event: '⚡',
		manual: '▶',
	};

	// --- Trigger summary helpers ---

	function summarizeTrigger(wf: RoleWorkflowEntry): string {
		const cfg = wf.triggerConfig || '';
		try {
			switch (wf.triggerType) {
				case 'schedule':
					return cronToHuman(cfg);
				case 'heartbeat':
					return intervalConfigToHuman(cfg);
				case 'event':
					return cfg ? `When ${cfg} fires` : 'On event';
				case 'manual':
					return 'Run manually';
				default:
					return wf.triggerType;
			}
		} catch {
			return wf.triggerType;
		}
	}

	function cronToHuman(cron: string): string {
		if (!cron) return 'Scheduled';
		const parts = cron.trim().split(/\s+/);
		if (parts.length !== 5) return cron;
		const [min, hour, , , dow] = parts;
		if (min === '*' || hour === '*') return cron;
		const time = formatTime(parseInt(hour), parseInt(min));
		if (dow === '*') return `Daily at ${time}`;
		if (dow === '1-5') return `Weekdays at ${time}`;
		if (dow === '0,6' || dow === '6,0') return `Weekends at ${time}`;
		const dayMap = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
		if (dow === '1') return `Mondays at ${time}`;
		if (dow === '5') return `Fridays at ${time}`;
		const dayNums = dow.split(',').map(Number).filter(n => !isNaN(n));
		if (dayNums.length > 0) {
			return `${dayNums.map(d => dayMap[d] || d).join(', ')} at ${time}`;
		}
		return cron;
	}

	function intervalConfigToHuman(cfg: string): string {
		if (!cfg) return 'Interval';
		const parts = cfg.split('|');
		const interval = parts[0];
		const window = parts[1];
		const base = intervalToHuman(interval);
		if (window) {
			const [start, end] = window.split('-');
			if (start && end) return `${base}, ${formatTime12(start)}–${formatTime12(end)}`;
		}
		return base;
	}

	function intervalToHuman(interval: string): string {
		if (interval === '5m') return 'Every 5 minutes';
		if (interval === '10m') return 'Every 10 minutes';
		if (interval === '15m') return 'Every 15 minutes';
		if (interval === '30m') return 'Every 30 minutes';
		if (interval === '1h') return 'Every hour';
		if (interval === '2h') return 'Every 2 hours';
		if (interval === '4h') return 'Every 4 hours';
		if (interval === '8h') return 'Every 8 hours';
		if (interval === '24h') return 'Every 24 hours';
		return `Every ${interval}`;
	}

	function formatTime(hour: number, min: number): string {
		const period = hour >= 12 ? 'PM' : 'AM';
		const h = hour > 12 ? hour - 12 : hour === 0 ? 12 : hour;
		const m = min.toString().padStart(2, '0');
		return `${h}:${m} ${period}`;
	}

	function formatTime12(time24: string): string {
		const [h, m] = time24.split(':').map(Number);
		if (isNaN(h)) return time24;
		const ampm = h >= 12 ? 'pm' : 'am';
		const h12 = h === 0 ? 12 : h > 12 ? h - 12 : h;
		if (m === 0) return `${h12}${ampm}`;
		return `${h12}:${m.toString().padStart(2, '0')}${ampm}`;
	}

	function lastFiredAgo(iso: string): string {
		const diff = Date.now() - new Date(iso).getTime();
		const mins = Math.floor(diff / 60000);
		if (mins < 1) return 'just now';
		if (mins < 60) return `${mins}m ago`;
		const hrs = Math.floor(mins / 60);
		if (hrs < 24) return `${hrs}h ago`;
		const days = Math.floor(hrs / 24);
		if (days < 7) return `${days}d ago`;
		return new Date(iso).toLocaleDateString();
	}

	// --- Emit helpers ---

	const emitMap = $derived.by(() => {
		const map: Record<string, string> = {};
		for (const wf of workflows) {
			if (wf.emit) map[wf.emit] = wf.description || wf.bindingName;
		}
		return map;
	});

	function getTriggeredBy(wf: RoleWorkflowEntry): string | null {
		if (wf.triggerType !== 'event') return null;
		const cfg = wf.triggerConfig || '';
		return emitMap[cfg] || null;
	}

	// --- Mode switching ---

	function switchMode(newMode: 'heartbeat' | 'automations') {
		mode = newMode;
	}

	// --- Heartbeat config ---

	async function saveHeartbeat(patch: Record<string, unknown>) {
		saving = true;
		try {
			const res = await updateEntityConfig(entityType, entityId, patch);
			if (res.config) {
				heartbeatEnabled = res.config.heartbeatEnabled ?? heartbeatEnabled;
				heartbeatInterval = res.config.heartbeatIntervalMinutes ?? heartbeatInterval;
				heartbeatWindow = res.config.heartbeatWindow ?? heartbeatWindow;
				heartbeatContent = res.config.heartbeatContent ?? heartbeatContent;
			}
		} catch {
			// ignore
		} finally {
			saving = false;
		}
	}

	function updateInterval(e: Event) {
		const val = Number((e.target as HTMLSelectElement).value);
		heartbeatInterval = val;
		saveHeartbeat({ heartbeatIntervalMinutes: val });
	}

	function updateWindowStart(e: Event) {
		const val = (e.target as HTMLInputElement).value;
		const end = heartbeatWindow?.[1] ?? '23:59';
		heartbeatWindow = [val, end];
		saveHeartbeat({ heartbeatWindowStart: val, heartbeatWindowEnd: end });
	}

	function updateWindowEnd(e: Event) {
		const start = heartbeatWindow?.[0] ?? '00:00';
		const val = (e.target as HTMLInputElement).value;
		heartbeatWindow = [start, val];
		saveHeartbeat({ heartbeatWindowStart: start, heartbeatWindowEnd: val });
	}

	let contentDebounce: ReturnType<typeof setTimeout> | null = null;
	function handleContentChange(val: string) {
		if (contentDebounce) clearTimeout(contentDebounce);
		contentDebounce = setTimeout(() => {
			saveHeartbeat({ heartbeatContent: val });
		}, 800);
	}

	// --- Workflow CRUD ---

	function openCreate() {
		editingWorkflow = null;
		showEditor = true;
	}

	function openEdit(wf: RoleWorkflowEntry) {
		editingWorkflow = wf;
		showEditor = true;
		overflowMenu = null;
	}

	async function handleDuplicate(wf: RoleWorkflowEntry) {
		if (!roleId) return;
		overflowMenu = null;
		try {
			const inputs = wf.inputs ? JSON.parse(wf.inputs) : undefined;
			const triggerConfig = wf.triggerConfig ? (() => {
				try { return JSON.parse(wf.triggerConfig); } catch {
					switch (wf.triggerType) {
						case 'schedule': return { cron: wf.triggerConfig };
						case 'heartbeat': {
							const parts = wf.triggerConfig.split('|');
							return { interval: parts[0], ...(parts[1] ? { window: parts[1] } : {}) };
						}
						case 'event': return { sources: wf.triggerConfig };
						default: return {};
					}
				}
			})() : {};
			await createRoleWorkflow(roleId, {
				bindingName: wf.bindingName + '-copy',
				triggerType: wf.triggerType,
				triggerConfig,
				description: (wf.description || wf.bindingName) + ' (copy)',
				inputs,
				emit: wf.emit,
			});
			await loadWorkflows();
		} catch {
			// ignore
		}
	}

	async function handleToggle(wf: RoleWorkflowEntry) {
		if (!roleId) return;
		toggling = wf.bindingName;
		try {
			await toggleRoleWorkflow(roleId, wf.bindingName);
			await loadWorkflows();
		} catch {
			// ignore
		} finally {
			toggling = null;
		}
	}

	async function handleDelete(bindingName: string) {
		if (!roleId) return;
		try {
			await deleteRoleWorkflow(roleId, bindingName);
			confirmDelete = null;
			overflowMenu = null;
			await loadWorkflows();
		} catch {
			// ignore
		}
	}

	function handleEditorSave() {
		showEditor = false;
		editingWorkflow = null;
		loadWorkflows();
	}

	function handleEditorClose() {
		showEditor = false;
		editingWorkflow = null;
	}

	// --- Data loading ---

	async function loadWorkflows() {
		if (!roleId) return;
		try {
			const res = await getRoleWorkflows(roleId);
			if (res?.workflows) workflows = res.workflows;
		} catch {
			// ignore
		}
	}

	async function loadAll() {
		loading = true;
		try {
			const configPromise = getEntityConfig(entityType, entityId).catch(() => null);
			const wfPromise = roleId ? getRoleWorkflows(roleId).catch(() => null) : Promise.resolve(null);
			const [configRes, wfRes] = await Promise.all([configPromise, wfPromise]);
			if (configRes?.config) {
				heartbeatEnabled = configRes.config.heartbeatEnabled ?? false;
				heartbeatInterval = configRes.config.heartbeatIntervalMinutes ?? 60;
				heartbeatWindow = configRes.config.heartbeatWindow ?? null;
				heartbeatContent = configRes.config.heartbeatContent ?? '';
				automationsEnabled = configRes.config.automationsEnabled ?? false;
			}
			if (wfRes?.workflows) workflows = wfRes.workflows;

			// Only set initial mode on first load — don't override user's selection
			if (!modeInitialized) {
				modeInitialized = true;
				if (workflows.length > 0) {
					mode = 'automations';
				} else {
					mode = 'heartbeat';
				}
			}
		} catch {
			// ignore
		} finally {
			loading = false;
		}
	}

	// Close overflow menu on outside click
	function handleWindowClick() {
		overflowMenu = null;
	}

	$effect(() => {
		void entityType;
		void entityId;
		modeInitialized = false;
		loadAll();
	});

	// Live-update lastFired via WS
	let wsUnsubs: Array<() => void> = [];
	onMount(() => {
		const ws = getWebSocketClient();
		wsUnsubs.push(
			ws.on('workflow_run_started', (data: { roleId: string }) => {
				if (roleId && data.roleId === roleId) loadWorkflows();
			}),
			ws.on('workflow_run_completed', (data: { roleId: string }) => {
				if (roleId && data.roleId === roleId) loadWorkflows();
			}),
			ws.on('workflow_run_failed', (data: { roleId: string }) => {
				if (roleId && data.roleId === roleId) loadWorkflows();
			}),
		);
	});
	onDestroy(() => {
		wsUnsubs.forEach(fn => fn());
	});
</script>

<svelte:window onclick={handleWindowClick} />

<section class="flex flex-col gap-5">

	{#if loading}
		<div class="flex justify-center py-8">
			<span class="loading loading-spinner loading-sm text-primary"></span>
		</div>

	{:else if !readonly}
		<!-- Automations list -->
			<div class="flex flex-col gap-3">
				<div class="flex items-center justify-between min-h-8">
					<h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">Automations</h2>
					{#if roleId}
						<button type="button" class="btn btn-sm btn-ghost text-primary gap-1.5" onclick={openCreate}>
							<Plus class="w-3.5 h-3.5" />
							New
						</button>
					{/if}
				</div>

				{#each workflows as wf}
					{@const triggeredBy = getTriggeredBy(wf)}
					{#if triggeredBy}
						<div class="flex items-center gap-2 pl-5 -my-1">
							<span class="text-xs text-base-content/80">&#8627; triggered by: {triggeredBy}</span>
						</div>
					{/if}
					<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
					<div class="rounded-xl border border-base-content/10 p-4 cursor-pointer hover:border-base-content/20 transition-colors" onclick={() => previewWorkflow = wf}>
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-3 min-w-0 flex-1">
								<div class="w-8 h-8 rounded-lg bg-base-content/5 flex items-center justify-center shrink-0 text-base">
									{triggerIcons[wf.triggerType] || '▶'}
								</div>
								<div class="min-w-0">
									<p class="text-sm font-medium truncate">{wf.description || wf.bindingName}</p>
									<p class="text-xs text-base-content/70 truncate">
										{summarizeTrigger(wf)}{#if wf.activities && wf.activities.length > 0}{' '}&middot; {wf.activities.length} step{wf.activities.length !== 1 ? 's' : ''}{/if}
									</p>
									{#if wf.lastFired}
										<p class="text-xs text-base-content/70 truncate mt-0.5">Last run: {lastFiredAgo(wf.lastFired)}</p>
									{/if}
									{#if wf.emit}
										<p class="text-xs text-base-content/70 truncate mt-0.5">&#8594; announces: {wf.emit}</p>
									{/if}
								</div>
							</div>
							<div class="flex items-center gap-1.5 shrink-0 ml-3">
								<div class="relative">
									<button
										type="button"
										class="btn btn-xs btn-ghost btn-square text-base-content/70 hover:text-base-content/80"
										onclick={(e) => { e.stopPropagation(); overflowMenu = overflowMenu === wf.bindingName ? null : wf.bindingName; }}
									>
										<MoreHorizontal class="w-3.5 h-3.5" />
									</button>
									{#if overflowMenu === wf.bindingName}
										<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
										<div
											class="absolute right-0 top-full mt-1 z-20 w-36 rounded-lg bg-base-100 border border-base-content/10 shadow-lg py-1"
											onclick={(e) => e.stopPropagation()}
										>
											<button type="button" class="w-full flex items-center gap-2 px-3 py-1.5 text-sm text-base-content/80 hover:bg-base-content/5 transition-colors" onclick={() => openEdit(wf)}>
												<Pencil class="w-3.5 h-3.5" /> Edit
											</button>
											<button type="button" class="w-full flex items-center gap-2 px-3 py-1.5 text-sm text-base-content/80 hover:bg-base-content/5 transition-colors" onclick={() => handleDuplicate(wf)}>
												<Copy class="w-3.5 h-3.5" /> Duplicate
											</button>
											<div class="border-t border-base-content/10 my-1"></div>
											{#if confirmDelete === wf.bindingName}
												<button type="button" class="w-full flex items-center gap-2 px-3 py-1.5 text-sm text-error hover:bg-error/5 transition-colors" onclick={() => handleDelete(wf.bindingName)}>
													<Trash2 class="w-3.5 h-3.5" /> Confirm delete
												</button>
											{:else}
												<button type="button" class="w-full flex items-center gap-2 px-3 py-1.5 text-sm text-error/70 hover:bg-error/5 hover:text-error transition-colors" onclick={(e) => { e.stopPropagation(); confirmDelete = wf.bindingName; }}>
													<Trash2 class="w-3.5 h-3.5" /> Delete
												</button>
											{/if}
										</div>
									{/if}
								</div>
								<input
									type="checkbox"
									class="toggle toggle-sm toggle-primary"
									checked={wf.isActive}
									disabled={toggling === wf.bindingName}
									onclick={(e) => e.stopPropagation()}
									onchange={() => handleToggle(wf)}
								/>
							</div>
						</div>
					</div>
				{/each}

				{#if workflows.length === 0}
					<div class="flex flex-col items-center py-8 text-center">
						<svg class="w-8 h-8 text-base-content/80 mb-2" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
							<circle cx="12" cy="12" r="10" /><polyline points="12 6 12 12 16 14" />
						</svg>
						<p class="text-sm text-base-content/70">No automations yet</p>
						<p class="text-xs text-base-content/70 mt-1 mb-3">Add one to put this agent on autopilot.</p>
						<button type="button" class="btn btn-sm btn-primary gap-1" onclick={openCreate}>
							<Plus class="w-3.5 h-3.5" />
							New Automation
						</button>
					</div>
				{/if}
			</div>

	{:else}
		<!-- Assistant readonly -->
		<div class="flex flex-col items-center py-6 text-center">
			<Store class="w-6 h-6 text-base-content/80 mb-2" />
			<p class="text-sm text-base-content/70">Workflow automations are available on installed agents.</p>
			<p class="text-xs text-base-content/70 mt-1">Browse the marketplace to add agents with built-in automations.</p>
		</div>
	{/if}

</section>

{#if previewWorkflow}
	{@const pw = previewWorkflow}
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div
		class="fixed inset-0 z-[60] flex flex-col bg-base-100"
		transition:fly={{ y: 20, duration: 200 }}
	>
		<header class="flex items-center justify-between px-6 h-14 border-b border-base-content/10 shrink-0">
			<div class="flex items-center gap-3 min-w-0">
				<button type="button" class="btn btn-sm btn-ghost btn-square" onclick={() => previewWorkflow = null}>
					<X class="w-4 h-4" />
				</button>
				<h2 class="text-base font-semibold truncate">{pw.description || pw.bindingName}</h2>
			</div>
			<button
				type="button"
				class="btn btn-sm btn-primary gap-1.5"
				onclick={() => { const wf = pw; previewWorkflow = null; openEdit(wf); }}
			>
				<Pencil class="w-3.5 h-3.5" />
				Edit
			</button>
		</header>

		<div class="flex-1 overflow-y-auto">
			<div class="max-w-3xl mx-auto px-6 py-8">
				<!-- Trigger -->
				<div class="mb-6">
					<span class="text-xs text-base-content/70 uppercase tracking-wider font-semibold">Trigger</span>
					<div class="flex items-center gap-2 mt-2">
						<span class="text-lg">{triggerIcons[pw.triggerType] || '▶'}</span>
						<span class="text-sm">{summarizeTrigger(pw)}</span>
					</div>
				</div>

				<!-- Steps -->
				{#if pw.activities && pw.activities.length > 0}
					<div class="mb-6">
						<span class="text-xs text-base-content/70 uppercase tracking-wider font-semibold">Steps</span>
						<ol class="mt-3 flex flex-col gap-3">
							{#each pw.activities as activity, i}
								<li class="flex gap-3">
									<div class="w-6 h-6 rounded-full bg-primary/10 text-primary flex items-center justify-center text-xs font-semibold shrink-0 mt-0.5">
										{i + 1}
									</div>
									<div class="min-w-0">
										<p class="text-sm font-medium">{activity.intent || activity.id || `Step ${i + 1}`}</p>
										{#if activity.skills && (activity.skills as string[]).length > 0}
											<p class="text-xs text-base-content/70 mt-0.5">Skills: {(activity.skills as string[]).join(', ')}</p>
										{/if}
										{#if activity.steps && (activity.steps as string[]).length > 0}
											<ul class="mt-1.5 flex flex-col gap-1">
												{#each activity.steps as step}
													<li class="text-xs text-base-content/80 flex items-start gap-1.5">
														<span class="text-base-content/80 mt-px">&#8226;</span>
														<span>{step}</span>
													</li>
												{/each}
											</ul>
										{/if}
									</div>
								</li>
							{/each}
						</ol>
					</div>
				{/if}

				<!-- Emit -->
				{#if pw.emit}
					<div class="mb-6">
						<span class="text-xs text-base-content/70 uppercase tracking-wider font-semibold">On completion</span>
						<p class="text-sm mt-2">Announces: <span class="font-medium">{pw.emit}</span></p>
					</div>
				{/if}

				<!-- Status -->
				<div>
					<span class="text-xs text-base-content/70 uppercase tracking-wider font-semibold">Status</span>
					<p class="text-sm mt-2">{pw.isActive ? 'Active' : 'Paused'}</p>
				</div>
			</div>
		</div>
	</div>
{/if}

{#if showEditor && roleId}
	<AutomationEditor
		roleId={roleId}
		existing={editingWorkflow}
		onclose={handleEditorClose}
		onsave={handleEditorSave}
	/>
{/if}
