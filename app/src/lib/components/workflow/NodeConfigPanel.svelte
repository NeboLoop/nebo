<script lang="ts">
	import { t, locale } from 'svelte-i18n';
	import EventSourcePicker from './EventSourcePicker.svelte';
	import { getActivityType, ACTIVITY_TYPES, type ActivityType } from '$lib/utils/workflowTypes';
	import type { WorkflowConfig, WorkflowActivity, WorkflowTrigger } from '$lib/types/agentPage';

	let {
		agentId = '',
		workflowName = '',
		workflow = null,
		selectedNodeId = null,
		activity = null,
		mode = 'view',
		onupdateActivity,
		onupdateTrigger,
		onupdateEmit,
		onupdateDescription,
		onupdateActive,
		onremove,
		onremoveWorkflow,
		onclose,
		ondismiss,
		onselectActivity,
	}: {
		agentId?: string;
		workflowName: string;
		workflow: WorkflowConfig | null;
		selectedNodeId: string | null;
		activity: WorkflowActivity | null;
		mode: 'view' | 'edit';
		onupdateActivity?: (field: keyof WorkflowActivity, value: unknown) => void;
		onupdateTrigger?: (trigger: WorkflowTrigger) => void;
		onupdateEmit?: (emit: string) => void;
		onupdateDescription?: (desc: string) => void;
		onupdateActive?: (active: boolean) => void;
		onremove?: (nodeId: string) => void;
		onremoveWorkflow?: () => void;
		onclose?: () => void;
		/** Mobile: dismiss the whole sheet (desktop keeps the panel column). */
		ondismiss?: () => void;
		onselectActivity?: (id: string) => void;
	} = $props();

	const isEditable = $derived(mode === 'edit');
	const triggerTypes = ['schedule', 'heartbeat', 'event', 'watch', 'manual'] as const;
	const triggerIcons: Record<string, string> = { schedule: '⏱', heartbeat: '♥', event: '⚡', watch: '👁', manual: '▶' };
	const activityTypeDef = $derived(activity ? getActivityType(activity.type) : null);

	// ── Schedule helpers
	const HOURS = Array.from({ length: 12 }, (_, i) => i + 1);
	const MINUTES = [0, 15, 30, 45];
	const DAY_LABELS = ['S', 'M', 'T', 'W', 'T', 'F', 'S'];
	const INTERVAL_OPTIONS = [
		{ value: '5m', labelKey: 'automations.every5min' },
		{ value: '10m', labelKey: 'automations.every10min' },
		{ value: '15m', labelKey: 'automations.every15min' },
		{ value: '30m', labelKey: 'automations.every30min' },
		{ value: '1h', labelKey: 'automations.everyHour' },
		{ value: '2h', labelKey: 'automations.every2h' },
		{ value: '4h', labelKey: 'automations.every4h' },
		{ value: '8h', labelKey: 'automations.every8h' },
		{ value: '24h', labelKey: 'automations.every24h' },
	];
	const DAY_PRESETS = [
		['daily', 'schedule.daily'],
		['weekdays', 'schedule.weekdays'],
		['weekends', 'workflow.schedule.weekends'],
		['custom', 'workflow.schedule.custom'],
	] as const;
	const CADENCES = [
		['weekly', 'workflow.schedule.daysOfWeek'],
		['monthly', 'workflow.schedule.monthly'],
		['biannual', 'workflow.schedule.biannual'],
		['annual', 'workflow.schedule.yearly'],
	] as const;
	
	// Display names come from Intl in the current locale; DAY_LABELS and
	// MONTH_LABELS stay the English tokens the schedule string is written in.
	/** Short month name for display (1-12). */
	function monthName(m: number): string {
		return new Intl.DateTimeFormat($locale ?? undefined, { month: 'short' }).format(new Date(2000, m - 1, 1));
	}
	/** One-letter weekday for display (0 = Sunday). 2 Jan 2000 was a Sunday. */
	function dayLetter(d: number): string {
		return new Intl.DateTimeFormat($locale ?? undefined, { weekday: 'narrow' }).format(new Date(2000, 0, 2 + d));
	}

	/** Parse "8:00 AM daily" or "3:00 PM weekdays" into structured parts */
	const MONTH_LABELS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

	function parseScheduleString(s: string): { hour: number; minute: number; ampm: string; days: string; customDays: number[]; cadence: string; dom: number; month: number } {
		const defaults = { hour: 8, minute: 0, ampm: 'AM', days: 'daily', customDays: [] as number[], cadence: 'weekly', dom: 1, month: 1 };
		if (!s) return defaults;
		const timeMatch = s.match(/(\d{1,2}):(\d{2})\s*(AM|PM)/i);
		if (timeMatch) {
			defaults.hour = parseInt(timeMatch[1]);
			defaults.minute = parseInt(timeMatch[2]);
			defaults.ampm = timeMatch[3].toUpperCase();
		}
		const lower = s.toLowerCase();
		// Month-anchored cadences round-trip through the strings buildScheduleString writes.
		const monthly = lower.match(/monthly on day (\d{1,2})/);
		const biannual = lower.match(/every 6 months on ([a-z]{3})\w* (\d{1,2})/);
		const yearly = lower.match(/yearly on ([a-z]{3})\w* (\d{1,2})/);
		const monthIdx = (name: string) => Math.max(0, MONTH_LABELS.findIndex(m => m.toLowerCase() === name));
		if (monthly) {
			return { ...defaults, days: 'custom', cadence: 'monthly', dom: parseInt(monthly[1]) };
		}
		if (biannual) {
			return { ...defaults, days: 'custom', cadence: 'biannual', month: monthIdx(biannual[1]) + 1, dom: parseInt(biannual[2]) };
		}
		if (yearly) {
			return { ...defaults, days: 'custom', cadence: 'annual', month: monthIdx(yearly[1]) + 1, dom: parseInt(yearly[2]) };
		}
		if (lower.includes('weekday')) defaults.days = 'weekdays';
		else if (lower.includes('weekend')) defaults.days = 'weekends';
		else if (lower.includes('daily') || lower.includes('every day')) defaults.days = 'daily';
		else if (lower.includes('monday') || lower.includes('mon ')) defaults.days = 'custom';
		else defaults.days = 'daily';
		return defaults;
	}

	/** The month 6 months after `m` (1-12). */
	function sixMonthsLater(m: number): number {
		return ((m + 5) % 12) + 1;
	}

	/** Build schedule string from structured parts */
	function buildScheduleString(hour: number, minute: number, ampm: string, days: string, customDays: number[], cadence: string, dom: number, month: number): string {
		const time = `${hour}:${minute.toString().padStart(2, '0')} ${ampm}`;
		if (days === 'custom' && cadence === 'monthly') return `${time} monthly on day ${dom}`;
		if (days === 'custom' && cadence === 'biannual') return `${time} every 6 months on ${MONTH_LABELS[month - 1]} ${dom}`;
		if (days === 'custom' && cadence === 'annual') return `${time} yearly on ${MONTH_LABELS[month - 1]} ${dom}`;
		if (days === 'weekdays') return `${time} weekdays`;
		if (days === 'weekends') return `${time} weekends`;
		if (days === 'daily') return `${time} daily`;
		if (days === 'custom' && customDays.length > 0) {
			const names = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
			return `${time} ${customDays.map(d => names[d]).join(', ')}`;
		}
		return `${time} daily`;
	}

	/** Build a 5-field cron from structured parts. Day-of-week uses named days
	 *  (MON-FRI) — numeric DOW is ambiguous between Unix and Quartz conventions.
	 *  Month-anchored cadences use the day-of-month and month fields instead. */
	function buildCron(hour: number, minute: number, ampm: string, days: string, customDays: number[], cadence: string, dom: number, month: number): string {
		let h = hour % 12;
		if (ampm === 'PM') h += 12;
		if (days === 'custom' && cadence === 'monthly') return `${minute} ${h} ${dom} * *`;
		if (days === 'custom' && cadence === 'biannual') {
			const months = [month, sixMonthsLater(month)].sort((a, b) => a - b);
			return `${minute} ${h} ${dom} ${months.join(',')} *`;
		}
		if (days === 'custom' && cadence === 'annual') return `${minute} ${h} ${dom} ${month} *`;
		const names = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT'];
		let dow = '*';
		if (days === 'weekdays') dow = 'MON-FRI';
		else if (days === 'weekends') dow = 'SAT,SUN';
		else if (days === 'custom' && customDays.length > 0) dow = customDays.map(d => names[d]).join(',');
		return `${minute} ${h} * * ${dow}`;
	}

	// ── Schedule editing state
	const schedParsed = $derived(parseScheduleString(workflow?.trigger?.schedule || workflow?.schedule || ''));
	let schedHour = $state(8);
	let schedMinute = $state(0);
	let schedAmpm = $state('AM');
	let schedDays = $state('daily');
	let schedCustomDays = $state<number[]>([]);
	let schedCadence = $state('weekly');
	let schedDom = $state(1);
	let schedMonth = $state(1);
	let schedInitFor = $state<string | null>(null);

	// Sync parsed schedule into editing state when switching workflows — keyed
	// by workflow name so tab switches / undo don't leak the previous
	// workflow's picker state into the next emitSchedule().
	$effect(() => {
		const p = schedParsed;
		if (schedInitFor !== workflowName || !isEditable) {
			schedHour = p.hour;
			schedMinute = p.minute;
			schedAmpm = p.ampm;
			schedDays = p.days;
			schedCustomDays = p.customDays;
			schedCadence = p.cadence;
			schedDom = p.dom;
			schedMonth = p.month;
			schedInitFor = workflowName;
		}
	});

	function emitSchedule() {
		const str = buildScheduleString(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays, schedCadence, schedDom, schedMonth);
		const cron = buildCron(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays, schedCadence, schedDom, schedMonth);
		onupdateTrigger?.({ ...currentTrigger(), schedule: str, cron });
	}

	/** Switch trigger type; preserves config when the type is unchanged. */
	// Watch editor data — plugins that declare events, and the chosen
	// plugin's event names. Loaded lazily on first entry to the watch editor.
	let watchPlugins = $state<{ slug: string; name: string }[]>([]);
	let watchEvents = $state<string[]>([]);
	let watchLoaded = $state(false);

	async function loadWatchPlugins() {
		if (watchLoaded) return;
		watchLoaded = true;
		try {
			const api = await import('$lib/api/nebo');
			const r = (await api.listAllPluginEvents()) as {
				events?: { plugin?: string; pluginName?: string; name?: string }[];
			};
			const seen = new Map<string, string>();
			for (const e of r.events ?? []) {
				if (e.plugin) seen.set(e.plugin, e.pluginName || e.plugin);
			}
			watchPlugins = [...seen.entries()].map(([slug, name]) => ({ slug, name }));
		} catch { watchPlugins = []; }
	}

	async function loadWatchEvents(slug: string) {
		watchEvents = [];
		if (!slug) return;
		try {
			const api = await import('$lib/api/nebo');
			const r = (await api.listPluginEvents(slug)) as { events?: { name?: string }[] };
			watchEvents = (r.events ?? []).map((e) => e.name || '').filter(Boolean);
		} catch { /* leave empty; the field stays a free input */ }
	}

	$effect(() => {
		if (workflow?.trigger?.type === 'watch' && isEditable) {
			loadWatchPlugins();
			if (workflow.trigger.plugin) loadWatchEvents(workflow.trigger.plugin);
		}
	});

	function switchTriggerType(tt: string) {
		if (workflow?.trigger?.type === tt) return;
		if (tt === 'schedule') {
			onupdateTrigger?.({
				type: tt,
				schedule: buildScheduleString(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays, schedCadence, schedDom, schedMonth),
				cron: buildCron(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays, schedCadence, schedDom, schedMonth),
			});
		} else if (tt === 'heartbeat') {
			onupdateTrigger?.({ type: tt, interval: '30m' });
		} else {
			onupdateTrigger?.({ type: tt });
		}
	}

	// ── Heartbeat editing state
	let hbWindowEnabled = $state(false);
	let hbInitFor = $state<string | null>(null);

	$effect(() => {
		if (hbInitFor !== workflowName || !isEditable) {
			const w = workflow?.trigger?.window;
			hbWindowEnabled = !!(w && (w.start || w.end));
			hbInitFor = workflowName;
		}
	});

	// ── Event source suggestions — the system knows every subscribable source
	// (workflow emits + watch-plugin auto-emissions); a typo'd source is a
	// subscription that silently never fires, so picking beats typing.
	let availableEventSources = $state<import('$lib/api/neboComponents').EventSourceOption[]>([]);
	let eventSourcesLoaded = $state(false);

	$effect(() => {
		if (workflow?.trigger?.type === 'event' && isEditable && !eventSourcesLoaded) {
			eventSourcesLoaded = true;
			import('$lib/api/nebo')
				.then((api) => api.listEventSources())
				.then((resp) => { availableEventSources = resp?.sources ?? []; })
				.catch(() => { /* suggestions are an enhancement, not a dependency */ });
		}
	});

	// ── Editing state for steps
	let editingStepIdx = $state<number | null>(null);
	let editingStepText = $state('');
	let newStepText = $state('');
	let newSkillText = $state('');

	/** Safe accessor: returns the current trigger or a default with required `type`. */
	function currentTrigger(): WorkflowTrigger {
		return workflow?.trigger ?? { type: 'manual' };
	}

	/** Display name for a trigger type; unknown types show as stored. */
	function triggerTypeLabel(type: string): string {
		return (triggerTypes as readonly string[]).includes(type) ? $t(`workflow.triggerTypes.${type}`) : type;
	}
	
	/** Display text for a heartbeat interval value. */
	function intervalLabel(interval: string): string {
		const opt = INTERVAL_OPTIONS.find((o) => o.value === interval);
		return opt ? $t(opt.labelKey) : $t('automations.everyInterval', { values: { interval } });
	}
	
	function formatLastFired(iso: string): string {
		const d = new Date(iso);
		return isNaN(d.getTime()) ? iso : d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
	}

	function updateParam(key: string, value: unknown) {
		const params = { ...(activity?.params || {}), [key]: value };
		onupdateActivity?.('params', params);
	}
</script>

<!-- On small screens the config panel is a bottom sheet over the canvas
     (70% height) rather than a fixed stacked band — the canvas keeps its
     full area when the sheet is closed. -->
<div class="w-[340px] shrink-0 border-s border-base-content/10 bg-base-100 flex flex-col overflow-hidden max-md:absolute max-md:inset-x-0 max-md:bottom-0 max-md:z-20 max-md:w-full max-md:h-[70%] max-md:border-s-0 max-md:border-t max-md:rounded-t-xl max-md:shadow-2xl">
	<!-- Panel header -->
	<div class="flex items-center justify-between px-4 py-3 border-b border-base-content/10 shrink-0">
		<!-- On a phone this panel is a sheet OVER the chain. Without its own
		     exit the only visible × closed the whole builder — a trap. -->
		{#if ondismiss}
			<button
				type="button"
				class="md:hidden me-2 h-9 px-3 rounded-md flex items-center gap-1 text-sm font-medium text-primary bg-transparent border-none cursor-pointer shrink-0"
				onclick={ondismiss}
			>
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
				{$t('common.done')}
			</button>
		{/if}
		<div class="flex-1 min-w-0">
			<div class="text-sm font-semibold truncate">{workflowName}</div>
			<div class="text-xs text-base-content/50">{$t('workflow.activityCount', { values: { count: workflow?.activities?.length ?? 0 } })}</div>
		</div>
		<div class="flex items-center gap-1.5 shrink-0">
			{#if isEditable}
				<input
					type="checkbox"
					class="toggle toggle-sm max-md:toggle-md toggle-primary"
					checked={workflow?.isActive !== false}
					role="switch"
					aria-checked={workflow?.isActive !== false}
					title={$t('workflowCanvas.enableDisable')}
					onchange={(e) => onupdateActive?.((e.target as HTMLInputElement).checked)}
				/>
			{/if}
			{#if selectedNodeId}
				<!-- Only meaningful with a node selected: returns to the
				     workflow overview. The panel itself is a fixed column —
				     a dead × in overview mode was a lying control. -->
				<button
					class="w-6 h-6 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base"
					title={$t('workflow.panel.backToOverview')}
					aria-label={$t('workflow.panel.backToOverview')}
					onclick={onclose}
				>&times;</button>
			{/if}
		</div>
	</div>

	<div class="flex-1 overflow-y-auto p-4">
		{#if selectedNodeId && activity}
			<!-- ═══ Activity detail ═══ -->

			<!-- Type badge -->
			{#if activityTypeDef}
				<div class="mb-3 flex items-center gap-2">
					<div class="w-6 h-6 rounded-md bg-base-200 flex items-center justify-center text-sm shrink-0">{activityTypeDef.icon}</div>
					<span class="text-sm font-medium text-base-content/70">{activityTypeDef.label}</span>
					{#if isEditable}
						<select
							class="select select-sm select-bordered ms-auto"
							value={activity.type || 'custom'}
							onchange={(e) => onupdateActivity?.('type', (e.target as HTMLSelectElement).value)}
						>
							{#each Object.keys(ACTIVITY_TYPES) as typeId}
								<option value={typeId}>{getActivityType(typeId).label}</option>
							{/each}
						</select>
					{/if}
				</div>
			{/if}

			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflowCanvas.activity')}</div>
				{#if isEditable}
					<input
						type="text"
						class="input input-sm input-bordered w-full font-medium"
						value={activity.id}
						onchange={(e) => onupdateActivity?.('id', (e.target as HTMLInputElement).value)}
					/>
				{:else}
					<div class="text-sm font-medium">{activity.id}</div>
				{/if}
			</div>

			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflow.panel.intent')}</div>
				{#if isEditable}
					<textarea
						class="textarea textarea-sm textarea-bordered w-full resize-none"
						rows="2"
						value={activity.intent}
						onchange={(e) => onupdateActivity?.('intent', (e.target as HTMLTextAreaElement).value)}
					></textarea>
				{:else}
					<div class="text-sm text-base-content/70 mt-0.5">{activity.intent}</div>
				{/if}
			</div>

			<!-- Skills -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('marketplace.skills')}</div>
				<div class="flex flex-wrap gap-1">
					{#each activity.skills ?? [] as skill, i}
						<div class="flex items-center gap-1 py-0.5 px-2 rounded bg-base-200 font-mono text-xs">
							<span class="truncate">{skill}</span>
							{#if isEditable}
								<button
									class="text-base-content/40 hover:text-error cursor-pointer bg-transparent border-none text-xs leading-none p-0"
									aria-label={$t('workflow.removeItem', { values: { name: skill } })}
									onclick={() => {
										const skills = [...(activity.skills || [])];
										skills.splice(i, 1);
										onupdateActivity?.('skills', skills);
									}}
								>&times;</button>
							{/if}
						</div>
					{/each}
				</div>
				{#if isEditable}
					<div class="flex gap-1 mt-1.5">
						<input
							type="text"
							class="input input-sm input-bordered flex-1"
							placeholder={$t('workflow.panel.addSkill')}
							bind:value={newSkillText}
							onkeydown={(e) => {
								if (e.key === 'Enter' && newSkillText.trim()) {
									const skills = [...(activity.skills || []), newSkillText.trim()];
									onupdateActivity?.('skills', skills);
									newSkillText = '';
								}
							}}
						/>
						<button
							class="btn btn-xs max-md:btn-sm btn-ghost"
							disabled={!newSkillText.trim()}
							onclick={() => {
								if (newSkillText.trim()) {
									const skills = [...(activity.skills || []), newSkillText.trim()];
									onupdateActivity?.('skills', skills);
									newSkillText = '';
								}
							}}
						>+</button>
					</div>
				{/if}
			</div>

			<!-- Type-specific parameters -->
			{#if activityTypeDef && activityTypeDef.parameters.length > 0}
				<div class="mb-4">
					<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('workflow.panel.parameters')}</div>
					<div class="flex flex-col gap-2">
						{#each activityTypeDef.parameters as param}
							<div>
								<label class="text-xs text-base-content/60 mb-0.5 block" for="param-{param.key}">{param.label}</label>
								{#if param.type === 'select'}
									{#if isEditable}
										<select
											id="param-{param.key}"
											class="select select-sm select-bordered w-full"
											value={String(activity.params?.[param.key] ?? param.default ?? '')}
											onchange={(e) => updateParam(param.key, (e.target as HTMLSelectElement).value)}
										>
											{#each param.options ?? [] as opt}
												<option value={opt.value}>{opt.label}</option>
											{/each}
										</select>
									{:else}
										<div class="text-xs font-mono text-base-content/70">{activity.params?.[param.key] ?? param.default ?? '—'}</div>
									{/if}
								{:else if param.type === 'textarea'}
									{#if isEditable}
										<textarea
											id="param-{param.key}"
											class="textarea textarea-sm textarea-bordered w-full resize-none"
											rows="2"
											placeholder={param.placeholder}
											value={String(activity.params?.[param.key] ?? '')}
											onchange={(e) => updateParam(param.key, (e.target as HTMLTextAreaElement).value)}
										></textarea>
									{:else}
										<div class="text-xs text-base-content/70">{activity.params?.[param.key] || '—'}</div>
									{/if}
								{:else if param.type === 'toggle'}
									<input
										id="param-{param.key}"
										type="checkbox"
										class="toggle toggle-xs toggle-primary"
										checked={Boolean(activity.params?.[param.key] ?? param.default ?? false)}
										disabled={!isEditable}
										onchange={(e) => updateParam(param.key, (e.target as HTMLInputElement).checked)}
									/>
								{:else}
									{#if isEditable}
										<input
											id="param-{param.key}"
											type={param.type === 'number' ? 'number' : 'text'}
											class="input input-sm input-bordered w-full"
											placeholder={param.placeholder}
											value={String(activity.params?.[param.key] ?? '')}
											onchange={(e) => {
												const raw = (e.target as HTMLInputElement).value;
												// Numbers stay numbers — "100" as a string breaks
												// maxIterations and numeric expression comparisons.
												updateParam(param.key, param.type === 'number' ? Number(raw) : raw);
											}}
										/>
									{:else}
										<div class="text-xs text-base-content/70 font-mono">{activity.params?.[param.key] || '—'}</div>
									{/if}
								{/if}
								{#if param.description}
									<div class="text-xs text-base-content/40 mt-0.5">{param.description}</div>
								{/if}
							</div>
						{/each}
					</div>
				</div>
			{/if}

			<!-- Steps -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('automations.steps')}</div>
				<div class="flex flex-col gap-1">
					{#each activity.steps ?? [] as step, i}
						<div class="flex items-start gap-2 py-1.5 px-2 rounded-md border border-base-300 bg-base-100 group">
							<span class="font-mono text-xs text-base-content/40 shrink-0 mt-px w-3 text-end">{i + 1}</span>
							{#if isEditable && editingStepIdx === i}
								<input
									type="text"
									class="input input-sm input-bordered flex-1"
									bind:value={editingStepText}
									onkeydown={(e) => {
										if (e.key === 'Enter') {
											const steps = [...(activity.steps || [])];
											steps[i] = editingStepText;
											onupdateActivity?.('steps', steps);
											editingStepIdx = null;
										}
										if (e.key === 'Escape') editingStepIdx = null;
									}}
									onblur={() => {
										const steps = [...(activity.steps || [])];
										steps[i] = editingStepText;
										onupdateActivity?.('steps', steps);
										editingStepIdx = null;
									}}
								/>
							{:else}
								{#if isEditable}
									<span
										class="text-sm flex-1 cursor-pointer hover:text-primary"
										role="button"
										tabindex="0"
										onclick={() => { editingStepIdx = i; editingStepText = step; }}
										onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); editingStepIdx = i; editingStepText = step; } }}
									>{step}</span>
								{:else}
									<span class="text-sm flex-1">{step}</span>
								{/if}
							{/if}
							{#if isEditable}
								<button
									class="text-base-content/30 hover:text-error cursor-pointer bg-transparent border-none text-xs leading-none p-0 opacity-0 group-hover:opacity-100 transition-opacity"
									aria-label={$t('common.remove')}
									onclick={() => {
										const steps = [...(activity.steps || [])];
										steps.splice(i, 1);
										onupdateActivity?.('steps', steps);
									}}
								>&times;</button>
							{/if}
						</div>
					{/each}
				</div>
				{#if isEditable}
					<div class="flex gap-1 mt-1.5">
						<input
							type="text"
							class="input input-sm input-bordered flex-1"
							placeholder={$t('workflow.panel.addStep')}
							bind:value={newStepText}
							onkeydown={(e) => {
								if (e.key === 'Enter' && newStepText.trim()) {
									const steps = [...(activity.steps || []), newStepText.trim()];
									onupdateActivity?.('steps', steps);
									newStepText = '';
								}
							}}
						/>
						<button
							class="btn btn-xs max-md:btn-sm btn-ghost"
							disabled={!newStepText.trim()}
							onclick={() => {
								if (newStepText.trim()) {
									const steps = [...(activity.steps || []), newStepText.trim()];
									onupdateActivity?.('steps', steps);
									newStepText = '';
								}
							}}
						>+</button>
					</div>
				{/if}
			</div>

			<!-- Delete button (edit mode) -->
			{#if isEditable && selectedNodeId !== '__trigger__'}
				<button
					class="btn btn-sm btn-error btn-outline w-full mt-2"
					onclick={() => { if (selectedNodeId) onremove?.(selectedNodeId); }}
				>{$t('workflow.deleteNode')}</button>
			{/if}

		{:else if selectedNodeId === null || selectedNodeId === '__trigger__' || selectedNodeId === '__emit__'}
			<!-- ═══ Workflow overview ═══ -->

			<!-- Trigger config -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('automations.trigger')}</div>
				{#if isEditable}
					<!-- Trigger type selector (4 buttons like v1) -->
					<div class="grid grid-cols-4 gap-1 mb-3">
						{#each triggerTypes as tt}
							<button
								class="flex flex-col items-center gap-0.5 py-2 px-1 rounded-lg border text-center cursor-pointer transition-colors
									{workflow?.trigger?.type === tt
										? 'border-primary bg-primary/10 text-primary'
										: 'border-base-300 bg-transparent hover:border-base-content/20 text-base-content/70'}"
								onclick={() => switchTriggerType(tt)}
							>
								<span class="text-sm">{triggerIcons[tt]}</span>
								<span class="text-xs font-medium">{$t(`workflow.triggerTypes.${tt}`)}</span>
							</button>
						{/each}
					</div>

					<!-- Schedule config -->
					{#if workflow?.trigger?.type === 'schedule'}
						<div class="flex flex-col gap-2">
							<!-- Time picker: Hour : Minute AM/PM -->
							<div class="flex items-center gap-1.5">
								<select
									class="select select-sm select-bordered w-16"
									value={schedHour}
									onchange={(e) => { schedHour = parseInt((e.target as HTMLSelectElement).value); emitSchedule(); }}
								>
									{#each HOURS as h}
										<option value={h}>{h}</option>
									{/each}
								</select>
								<span class="text-xs text-base-content/40">:</span>
								<select
									class="select select-sm select-bordered w-16"
									value={schedMinute}
									onchange={(e) => { schedMinute = parseInt((e.target as HTMLSelectElement).value); emitSchedule(); }}
								>
									{#each MINUTES as m}
										<option value={m}>{m.toString().padStart(2, '0')}</option>
									{/each}
								</select>
								<div class="flex border border-base-300 rounded-lg overflow-hidden">
									<button
										class="px-2 py-1 text-xs font-medium cursor-pointer border-none transition-colors
											{schedAmpm === 'AM' ? 'bg-primary/10 text-primary' : 'bg-transparent text-base-content/50 hover:text-base-content/70'}"
										onclick={() => { schedAmpm = 'AM'; emitSchedule(); }}
									>{$t('schedule.am')}</button>
									<button
										class="px-2 py-1 text-xs font-medium cursor-pointer border-none transition-colors
											{schedAmpm === 'PM' ? 'bg-primary/10 text-primary' : 'bg-transparent text-base-content/50 hover:text-base-content/70'}"
										onclick={() => { schedAmpm = 'PM'; emitSchedule(); }}
									>{$t('schedule.pm')}</button>
								</div>
							</div>

							<!-- Day presets -->
							<div class="flex gap-1">
								{#each DAY_PRESETS as [val, labelKey]}
									<button
										class="flex-1 py-1 text-xs font-medium rounded-md border cursor-pointer transition-colors
											{schedDays === val
												? 'border-primary bg-primary/10 text-primary'
												: 'border-base-300 bg-transparent text-base-content/60 hover:border-base-content/20'}"
										onclick={() => { schedDays = val; emitSchedule(); }}
									>{$t(labelKey)}</button>
								{/each}
							</div>

							<!-- Custom: pick a cadence, then its anchor -->
							{#if schedDays === 'custom'}
								<div class="flex gap-1">
									{#each CADENCES as [val, labelKey]}
										<button
											class="flex-1 py-1 text-xs font-medium rounded-md border cursor-pointer transition-colors
												{schedCadence === val
													? 'border-primary bg-primary/10 text-primary'
													: 'border-base-300 bg-transparent text-base-content/60 hover:border-base-content/20'}"
											onclick={() => { schedCadence = val; emitSchedule(); }}
										>{$t(labelKey)}</button>
									{/each}
								</div>
								{#if schedCadence === 'weekly'}
									<div class="flex gap-1">
										{#each DAY_LABELS as _d, i}
											<button
												class="w-8 h-8 max-md:w-10 max-md:h-10 rounded-full text-xs font-medium border cursor-pointer transition-colors
													{schedCustomDays.includes(i)
														? 'border-primary bg-primary/10 text-primary'
														: 'border-base-300 bg-transparent text-base-content/50 hover:border-base-content/20'}"
												onclick={() => {
													schedCustomDays = schedCustomDays.includes(i)
														? schedCustomDays.filter(x => x !== i)
														: [...schedCustomDays, i].sort();
													emitSchedule();
												}}
											>{dayLetter(i)}</button>
										{/each}
									</div>
								{:else}
									<div class="flex items-center gap-2">
										{#if schedCadence !== 'monthly'}
											<select
												class="select select-sm select-bordered"
												value={schedMonth}
												onchange={(e) => { schedMonth = parseInt((e.target as HTMLSelectElement).value); emitSchedule(); }}
											>
												{#each MONTH_LABELS as _m, i}
													<option value={i + 1}>{monthName(i + 1)}</option>
												{/each}
											</select>
										{/if}
										<span class="text-xs text-base-content/60">{$t('workflow.schedule.onDay')}</span>
										<select
											class="select select-sm select-bordered w-16"
											value={schedDom}
											onchange={(e) => { schedDom = parseInt((e.target as HTMLSelectElement).value); emitSchedule(); }}
										>
											{#each Array.from({ length: 31 }, (_, i) => i + 1) as d}
												<option value={d}>{d}</option>
											{/each}
										</select>
									</div>
									{#if schedCadence === 'biannual'}
										<div class="text-xs text-base-content/40">
											{$t('workflow.schedule.biannualNote', { values: { firstMonth: monthName(schedMonth), secondMonth: monthName(((schedMonth + 5) % 12) + 1), day: schedDom } })}
										</div>
									{/if}
									{#if schedDom > 28}
										<div class="text-xs text-base-content/40">{$t('workflow.schedule.shortMonthsSkipped', { values: { day: schedDom } })}</div>
									{/if}
								{/if}
							{/if}
						</div>
					{/if}

					<!-- Heartbeat config -->
					{#if workflow?.trigger?.type === 'heartbeat'}
						<div class="flex flex-col gap-2">
							<!-- Interval dropdown -->
							<div>
								<label class="text-xs text-base-content/60 mb-0.5 block" for="hb-interval">{$t('schedule.every')}</label>
								<select
									id="hb-interval"
									class="select select-sm select-bordered w-full"
									value={workflow?.trigger?.interval || '30m'}
									onchange={(e) => onupdateTrigger?.({ ...currentTrigger(), interval: (e.target as HTMLSelectElement).value })}
								>
									{#each INTERVAL_OPTIONS as opt}
										<option value={opt.value}>{$t(opt.labelKey)}</option>
									{/each}
								</select>
							</div>

							<!-- Time window -->
							<div>
								<label class="flex items-center gap-2 cursor-pointer">
									<input
										type="checkbox"
										class="checkbox checkbox-xs checkbox-primary"
										checked={hbWindowEnabled}
										onchange={(e) => {
											hbWindowEnabled = (e.target as HTMLInputElement).checked;
											if (!hbWindowEnabled) {
												onupdateTrigger?.({ ...currentTrigger(), window: undefined });
											} else {
												onupdateTrigger?.({ ...currentTrigger(), window: { start: '09:00', end: '18:00' } });
											}
										}}
									/>
									<span class="text-xs text-base-content/60">{$t('workflow.schedule.limitToHours')}</span>
								</label>
							</div>
							{#if hbWindowEnabled}
								<div class="flex items-center gap-2">
									<input
										type="time"
										class="input input-sm input-bordered flex-1"
										value={workflow?.trigger?.window?.start || '09:00'}
										onchange={(e) => onupdateTrigger?.({ ...currentTrigger(), window: { ...workflow?.trigger?.window, start: (e.target as HTMLInputElement).value } })}
									/>
									<span class="text-xs text-base-content/40">{$t('workflow.schedule.rangeTo')}</span>
									<input
										type="time"
										class="input input-sm input-bordered flex-1"
										value={workflow?.trigger?.window?.end || '18:00'}
										onchange={(e) => onupdateTrigger?.({ ...currentTrigger(), window: { ...workflow?.trigger?.window, end: (e.target as HTMLInputElement).value } })}
									/>
								</div>
							{/if}
						</div>
					{/if}

					<!-- Event config -->
					{#if workflow?.trigger?.type === 'event'}
						<div>
							<div class="text-xs text-base-content/60 mb-0.5">{$t('workflow.eventSources.label')}</div>
							<EventSourcePicker
								value={workflow?.trigger?.event || ''}
								suggestions={availableEventSources}
								onchange={(value) => onupdateTrigger?.({ ...currentTrigger(), event: value })}
							/>
							<div class="text-xs text-base-content/40 mt-1">{$t('workflow.eventSources.hint')}</div>
						</div>
					{/if}

					<!-- Watch: poll a plugin and run when it reports something new.
					     Two dropdowns, nothing to type — the plugin's own manifest
					     supplies the events and the command. -->
					{#if workflow?.trigger?.type === 'watch'}
						<div class="flex flex-col gap-2">
							<div>
								<div class="text-xs text-base-content/60 mb-0.5">{$t('workflow.watch.plugin')}</div>
								<select
									class="select select-sm w-full bg-base-100 border-base-300"
									value={workflow?.trigger?.plugin ?? ''}
									onchange={(e) => {
										const plugin = e.currentTarget.value;
										loadWatchEvents(plugin);
										onupdateTrigger?.({ type: 'watch', plugin, event: '' });
									}}
								>
									<option value="" disabled>{$t('workflow.watch.choosePlugin')}</option>
									{#each watchPlugins as pl (pl.slug)}
										<option value={pl.slug}>{pl.name}</option>
									{/each}
								</select>
							</div>
							{#if workflow?.trigger?.plugin}
								<div>
									<div class="text-xs text-base-content/60 mb-0.5">{$t('workflow.watch.whenItReports')}</div>
									<select
										class="select select-sm w-full bg-base-100 border-base-300"
										value={workflow?.trigger?.event ?? ''}
										onchange={(e) => onupdateTrigger?.({ type: 'watch', plugin: workflow?.trigger?.plugin ?? '', event: e.currentTarget.value })}
									>
										<option value="" disabled>{$t('workflow.watch.chooseEvent')}</option>
										{#each watchEvents as ev (ev)}
											<option value={ev}>{ev}</option>
										{/each}
									</select>
								</div>
								{#if workflow?.trigger?.event}
									<div class="text-xs text-base-content/40">{$t('workflow.watch.hint', { values: { plugin: workflow.trigger.plugin, event: workflow.trigger.event } })}</div>
								{/if}
							{/if}
						</div>
					{/if}

					<!-- Manual: no config -->
					{#if workflow?.trigger?.type === 'manual'}
						<div class="text-xs text-base-content/40">{$t('workflow.panel.manualHint')}</div>
					{/if}
				{:else}
					<!-- View mode -->
					<div class="flex items-center gap-2">
						<span class="text-sm">{triggerIcons[workflow?.trigger?.type ?? 'manual']}</span>
						<span class="text-sm font-medium capitalize">{triggerTypeLabel(workflow?.trigger?.type ?? 'manual')}</span>
					</div>
					{#if workflow?.trigger?.type === 'schedule'}
						<div class="text-xs text-base-content/50 font-mono mt-1">{workflow?.trigger?.schedule || workflow?.schedule || $t('workflow.panel.notConfigured')}</div>
					{:else if workflow?.trigger?.type === 'heartbeat'}
						<div class="text-xs text-base-content/50 font-mono mt-1">
							{intervalLabel(workflow?.trigger?.interval || '30m')}{#if workflow?.trigger?.window}, {workflow.trigger.window.start}–{workflow.trigger.window.end}{/if}
						</div>
					{:else if workflow?.trigger?.type === 'event'}
						<div class="text-xs text-base-content/50 font-mono mt-1">{workflow?.trigger?.event || $t('workflow.panel.noEventConfigured')}</div>
					{/if}
				{/if}
			</div>

			<!-- Description -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('common.description')}</div>
				{#if isEditable}
					<textarea
						class="textarea textarea-sm textarea-bordered w-full resize-none"
						rows="2"
						value={workflow?.description || ''}
						onchange={(e) => onupdateDescription?.((e.target as HTMLTextAreaElement).value)}
					></textarea>
				{:else}
					<div class="text-sm text-base-content/70 leading-relaxed">{workflow?.description || $t('workflow.panel.noDescription')}</div>
				{/if}
			</div>

			<!-- Emit config -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflowCanvas.emits')}</div>
				{#if isEditable}
					{@const suggestedEmit = `${workflowName.toLowerCase().replace(/\s+/g, '-')}.complete`}
					<input
						type="text"
						class="input input-sm input-bordered w-full font-mono"
						placeholder={$t('workflow.panel.emitPlaceholder', { values: { name: suggestedEmit } })}
						value={workflow?.emit || ''}
						onchange={(e) => onupdateEmit?.((e.target as HTMLInputElement).value)}
					/>
					{#if !workflow?.emit}
						<div class="text-xs text-base-content/40 mt-1">
							{$t('workflow.panel.emitOptional')}
							<button
								class="text-primary font-mono cursor-pointer bg-transparent border-none p-0 hover:underline"
								onclick={() => onupdateEmit?.(suggestedEmit)}
							>{$t('workflow.panel.useEmit', { values: { name: suggestedEmit } })}</button>
						</div>
					{:else}
						<div class="text-xs text-base-content/40 mt-1">{$t('workflow.panel.emitRenameWarning')}</div>
					{/if}
				{:else if workflow?.emit}
					<div class="py-1 px-2 rounded bg-accent/10 text-xs text-accent font-mono inline-block">{workflow.emit}</div>
				{:else}
					<div class="text-xs text-base-content/40">{$t('workflow.panel.none')}</div>
				{/if}
			</div>

			{#if workflow?.lastFired}
				<div class="mb-4">
					<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflowCanvas.lastFired')}</div>
					<div class="text-xs text-base-content/70 font-mono">{formatLastFired(workflow.lastFired)}</div>
				</div>
			{/if}

			<!-- Activity list -->
			<div>
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('workflowCanvas.activities')}</div>
				<div class="flex flex-col gap-1.5">
					{#each workflow?.activities ?? [] as act, idx}
						{@const td = getActivityType(act.type)}
						<button
							class="w-full flex items-start gap-2.5 p-2.5 rounded-lg border text-start cursor-pointer transition-colors bg-transparent
								{selectedNodeId === act.id ? 'border-primary bg-primary/5' : 'border-base-300 hover:border-base-content/20'}"
							onclick={() => onselectActivity?.(act.id)}
						>
							<div class="w-5 h-5 rounded-md bg-base-200 flex items-center justify-center text-xs shrink-0">{td.icon}</div>
							<div class="flex-1 min-w-0">
								<div class="text-sm font-medium truncate">{act.id}</div>
								<div class="text-xs text-base-content/60 truncate">{act.intent}</div>
								<div class="flex items-center gap-2 mt-0.5">
									{#if act.type && act.type !== 'custom'}
										<span class="text-xs text-base-content/50 font-mono">{td.label}</span>
									{/if}
									<span class="text-xs text-base-content/40 font-mono">{$t('workflow.stepCount', { values: { count: act.steps?.length ?? 0 } })}</span>
								</div>
							</div>
						</button>
					{/each}
				</div>
			</div>

			<!-- Endpoints are minted in the employee's Settings → Webhooks, the one
			     place every way of reaching this employee is listed and revoked. -->
			<div class="mt-4 pt-4 border-t border-base-content/10">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflow.panel.apiEndpoint')}</div>
				<div class="text-xs text-base-content/60">{$t('workflow.panel.apiEndpointHint')}</div>
			</div>

			<!-- Delete workflow (edit mode) -->
			{#if isEditable}
				<button
					class="btn btn-sm btn-error btn-outline w-full mt-4"
					onclick={() => onremoveWorkflow?.()}
				>{$t('workflow.deleteWorkflow')}</button>
			{/if}
		{/if}
	</div>

	<!-- Panel footer -->
	{#if selectedNodeId && activity}
		<div class="px-4 py-3 border-t border-base-content/10 shrink-0">
			<button
				class="text-xs text-primary cursor-pointer bg-transparent border-none hover:underline p-0"
				onclick={() => onselectActivity?.('')}
			>{$t('workflow.panel.backToOverview')}</button>
		</div>
	{/if}
</div>
