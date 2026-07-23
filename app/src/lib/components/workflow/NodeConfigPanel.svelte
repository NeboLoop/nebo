<script lang="ts">
	import EventSourcePicker from './EventSourcePicker.svelte';
	import { getActivityType, ACTIVITY_TYPES, type ActivityType } from '$lib/utils/workflowTypes';
	import type { WorkflowConfig, WorkflowActivity, WorkflowTrigger } from '$lib/types/agentPage';

	let {
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
		onselectActivity,
	}: {
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
		onselectActivity?: (id: string) => void;
	} = $props();

	const isEditable = $derived(mode === 'edit');
	const triggerTypes = ['schedule', 'heartbeat', 'event', 'manual'] as const;
	const triggerIcons: Record<string, string> = { schedule: '⏱', heartbeat: '♥', event: '⚡', manual: '▶' };
	const activityTypeDef = $derived(activity ? getActivityType(activity.type) : null);

	// ── Schedule helpers
	const HOURS = Array.from({ length: 12 }, (_, i) => i + 1);
	const MINUTES = [0, 15, 30, 45];
	const DAY_LABELS = ['S', 'M', 'T', 'W', 'T', 'F', 'S'];
	const INTERVAL_OPTIONS = [
		{ value: '5m', label: 'Every 5 minutes' },
		{ value: '10m', label: 'Every 10 minutes' },
		{ value: '15m', label: 'Every 15 minutes' },
		{ value: '30m', label: 'Every 30 minutes' },
		{ value: '1h', label: 'Every hour' },
		{ value: '2h', label: 'Every 2 hours' },
		{ value: '4h', label: 'Every 4 hours' },
		{ value: '8h', label: 'Every 8 hours' },
		{ value: '24h', label: 'Every 24 hours' },
	];

	/** Parse "8:00 AM daily" or "3:00 PM weekdays" into structured parts */
	function parseScheduleString(s: string): { hour: number; minute: number; ampm: string; days: string; customDays: number[] } {
		const defaults = { hour: 8, minute: 0, ampm: 'AM', days: 'daily', customDays: [] as number[] };
		if (!s) return defaults;
		const timeMatch = s.match(/(\d{1,2}):(\d{2})\s*(AM|PM)/i);
		if (timeMatch) {
			defaults.hour = parseInt(timeMatch[1]);
			defaults.minute = parseInt(timeMatch[2]);
			defaults.ampm = timeMatch[3].toUpperCase();
		}
		const lower = s.toLowerCase();
		if (lower.includes('weekday')) defaults.days = 'weekdays';
		else if (lower.includes('weekend')) defaults.days = 'weekends';
		else if (lower.includes('daily') || lower.includes('every day')) defaults.days = 'daily';
		else if (lower.includes('monday') || lower.includes('mon ')) defaults.days = 'custom';
		else defaults.days = 'daily';
		return defaults;
	}

	/** Build schedule string from structured parts */
	function buildScheduleString(hour: number, minute: number, ampm: string, days: string, customDays: number[]): string {
		const time = `${hour}:${minute.toString().padStart(2, '0')} ${ampm}`;
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
	 *  (MON-FRI) — numeric DOW is ambiguous between Unix and Quartz conventions. */
	function buildCron(hour: number, minute: number, ampm: string, days: string, customDays: number[]): string {
		let h = hour % 12;
		if (ampm === 'PM') h += 12;
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
			schedInitFor = workflowName;
		}
	});

	function emitSchedule() {
		const str = buildScheduleString(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays);
		const cron = buildCron(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays);
		onupdateTrigger?.({ ...currentTrigger(), schedule: str, cron });
	}

	/** Switch trigger type; preserves config when the type is unchanged. */
	function switchTriggerType(tt: string) {
		if (workflow?.trigger?.type === tt) return;
		if (tt === 'schedule') {
			onupdateTrigger?.({
				type: tt,
				schedule: buildScheduleString(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays),
				cron: buildCron(schedHour, schedMinute, schedAmpm, schedDays, schedCustomDays),
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

	function formatLastFired(iso: string): string {
		const d = new Date(iso);
		return isNaN(d.getTime()) ? iso : d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
	}

	function updateParam(key: string, value: unknown) {
		const params = { ...(activity?.params || {}), [key]: value };
		onupdateActivity?.('params', params);
	}
</script>

<div class="w-[340px] shrink-0 border-l border-base-content/10 bg-base-100 flex flex-col overflow-hidden max-md:w-full max-md:h-[40%] max-md:border-l-0 max-md:border-t">
	<!-- Panel header -->
	<div class="flex items-center justify-between px-4 py-3 border-b border-base-content/10 shrink-0">
		<div class="flex-1 min-w-0">
			<div class="text-sm font-semibold truncate">{workflowName}</div>
			<div class="text-xs text-base-content/50">{workflow?.activities?.length ?? 0} {(workflow?.activities?.length ?? 0) === 1 ? 'activity' : 'activities'}</div>
		</div>
		<div class="flex items-center gap-1.5 shrink-0">
			{#if isEditable}
				<input
					type="checkbox"
					class="toggle toggle-sm toggle-primary"
					checked={workflow?.isActive !== false}
					role="switch"
					aria-checked={workflow?.isActive !== false}
					title="Enable/disable"
					onchange={(e) => onupdateActive?.((e.target as HTMLInputElement).checked)}
				/>
			{/if}
			{#if selectedNodeId}
				<!-- Only meaningful with a node selected: returns to the
				     workflow overview. The panel itself is a fixed column —
				     a dead × in overview mode was a lying control. -->
				<button
					class="w-6 h-6 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base"
					title="Back to workflow overview"
					aria-label="Back to workflow overview"
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
							class="select select-sm select-bordered ml-auto"
							value={activity.type || 'custom'}
							onchange={(e) => onupdateActivity?.('type', (e.target as HTMLSelectElement).value)}
						>
							{#each Object.values(ACTIVITY_TYPES) as t}
								<option value={t.type}>{t.label}</option>
							{/each}
						</select>
					{/if}
				</div>
			{/if}

			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Activity</div>
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
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Intent</div>
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
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Skills</div>
				<div class="flex flex-wrap gap-1">
					{#each activity.skills ?? [] as skill, i}
						<div class="flex items-center gap-1 py-0.5 px-2 rounded bg-base-200 font-mono text-xs">
							<span class="truncate">{skill}</span>
							{#if isEditable}
								<button
									class="text-base-content/40 hover:text-error cursor-pointer bg-transparent border-none text-xs leading-none p-0"
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
							placeholder="Add skill..."
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
							class="btn btn-xs btn-ghost"
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
					<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Parameters</div>
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
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Steps</div>
				<div class="flex flex-col gap-1">
					{#each activity.steps ?? [] as step, i}
						<div class="flex items-start gap-2 py-1.5 px-2 rounded-md border border-base-300 bg-base-100 group">
							<span class="font-mono text-xs text-base-content/40 shrink-0 mt-px w-3 text-right">{i + 1}</span>
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
							placeholder="Add step..."
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
							class="btn btn-xs btn-ghost"
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
				>Delete Node</button>
			{/if}

		{:else if selectedNodeId === null || selectedNodeId === '__trigger__' || selectedNodeId === '__emit__'}
			<!-- ═══ Workflow overview ═══ -->

			<!-- Trigger config -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Trigger</div>
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
								<span class="text-xs font-medium">{tt.charAt(0).toUpperCase() + tt.slice(1)}</span>
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
									>AM</button>
									<button
										class="px-2 py-1 text-xs font-medium cursor-pointer border-none transition-colors
											{schedAmpm === 'PM' ? 'bg-primary/10 text-primary' : 'bg-transparent text-base-content/50 hover:text-base-content/70'}"
										onclick={() => { schedAmpm = 'PM'; emitSchedule(); }}
									>PM</button>
								</div>
							</div>

							<!-- Day presets -->
							<div class="flex gap-1">
								{#each [['daily', 'Daily'], ['weekdays', 'Weekdays'], ['weekends', 'Weekends'], ['custom', 'Custom']] as [val, label]}
									<button
										class="flex-1 py-1 text-xs font-medium rounded-md border cursor-pointer transition-colors
											{schedDays === val
												? 'border-primary bg-primary/10 text-primary'
												: 'border-base-300 bg-transparent text-base-content/60 hover:border-base-content/20'}"
										onclick={() => { schedDays = val; emitSchedule(); }}
									>{label}</button>
								{/each}
							</div>

							<!-- Custom day picker -->
							{#if schedDays === 'custom'}
								<div class="flex gap-1">
									{#each DAY_LABELS as d, i}
										<button
											class="w-8 h-8 rounded-full text-xs font-medium border cursor-pointer transition-colors
												{schedCustomDays.includes(i)
													? 'border-primary bg-primary/10 text-primary'
													: 'border-base-300 bg-transparent text-base-content/50 hover:border-base-content/20'}"
											onclick={() => {
												schedCustomDays = schedCustomDays.includes(i)
													? schedCustomDays.filter(x => x !== i)
													: [...schedCustomDays, i].sort();
												emitSchedule();
											}}
										>{d}</button>
									{/each}
								</div>
							{/if}
						</div>
					{/if}

					<!-- Heartbeat config -->
					{#if workflow?.trigger?.type === 'heartbeat'}
						<div class="flex flex-col gap-2">
							<!-- Interval dropdown -->
							<div>
								<label class="text-xs text-base-content/60 mb-0.5 block" for="hb-interval">Every</label>
								<select
									id="hb-interval"
									class="select select-sm select-bordered w-full"
									value={workflow?.trigger?.interval || '30m'}
									onchange={(e) => onupdateTrigger?.({ ...currentTrigger(), interval: (e.target as HTMLSelectElement).value })}
								>
									{#each INTERVAL_OPTIONS as opt}
										<option value={opt.value}>{opt.label}</option>
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
									<span class="text-xs text-base-content/60">Limit to hours</span>
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
									<span class="text-xs text-base-content/40">to</span>
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
							<div class="text-xs text-base-content/60 mb-0.5">Event sources</div>
							<EventSourcePicker
								value={workflow?.trigger?.event || ''}
								suggestions={availableEventSources}
								onchange={(value) => onupdateTrigger?.({ ...currentTrigger(), event: value })}
							/>
							<div class="text-xs text-base-content/40 mt-1">Type to search known sources, Enter to add. Custom names and wildcards (email.*) work too.</div>
						</div>
					{/if}

					<!-- Manual: no config -->
					{#if workflow?.trigger?.type === 'manual'}
						<div class="text-xs text-base-content/40">Runs only when manually triggered.</div>
					{/if}
				{:else}
					<!-- View mode -->
					<div class="flex items-center gap-2">
						<span class="text-sm">{triggerIcons[workflow?.trigger?.type ?? 'manual']}</span>
						<span class="text-sm font-medium capitalize">{workflow?.trigger?.type ?? 'manual'}</span>
					</div>
					{#if workflow?.trigger?.type === 'schedule'}
						<div class="text-xs text-base-content/50 font-mono mt-1">{workflow?.trigger?.schedule || workflow?.schedule || 'Not configured'}</div>
					{:else if workflow?.trigger?.type === 'heartbeat'}
						<div class="text-xs text-base-content/50 font-mono mt-1">
							{INTERVAL_OPTIONS.find(o => o.value === workflow?.trigger?.interval)?.label || `Every ${workflow?.trigger?.interval || '30m'}`}{#if workflow?.trigger?.window}, {workflow.trigger.window.start}–{workflow.trigger.window.end}{/if}
						</div>
					{:else if workflow?.trigger?.type === 'event'}
						<div class="text-xs text-base-content/50 font-mono mt-1">{workflow?.trigger?.event || 'No event configured'}</div>
					{/if}
				{/if}
			</div>

			<!-- Description -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Description</div>
				{#if isEditable}
					<textarea
						class="textarea textarea-sm textarea-bordered w-full resize-none"
						rows="2"
						value={workflow?.description || ''}
						onchange={(e) => onupdateDescription?.((e.target as HTMLTextAreaElement).value)}
					></textarea>
				{:else}
					<div class="text-sm text-base-content/70 leading-relaxed">{workflow?.description || 'No description'}</div>
				{/if}
			</div>

			<!-- Emit config -->
			<div class="mb-4">
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Emits</div>
				{#if isEditable}
					{@const suggestedEmit = `${workflowName.toLowerCase().replace(/\s+/g, '-')}.complete`}
					<input
						type="text"
						class="input input-sm input-bordered w-full font-mono"
						placeholder="e.g. {suggestedEmit}"
						value={workflow?.emit || ''}
						onchange={(e) => onupdateEmit?.((e.target as HTMLInputElement).value)}
					/>
					{#if !workflow?.emit}
						<div class="text-xs text-base-content/40 mt-1">
							Optional — other workflows can trigger on this when the run completes.
							<button
								class="text-primary font-mono cursor-pointer bg-transparent border-none p-0 hover:underline"
								onclick={() => onupdateEmit?.(suggestedEmit)}
							>Use {suggestedEmit}</button>
						</div>
					{:else}
						<div class="text-xs text-base-content/40 mt-1">Renaming breaks workflows subscribed to this event.</div>
					{/if}
				{:else if workflow?.emit}
					<div class="py-1 px-2 rounded bg-accent/10 text-xs text-accent font-mono inline-block">{workflow.emit}</div>
				{:else}
					<div class="text-xs text-base-content/40">None</div>
				{/if}
			</div>

			{#if workflow?.lastFired}
				<div class="mb-4">
					<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Last Fired</div>
					<div class="text-xs text-base-content/70 font-mono">{formatLastFired(workflow.lastFired)}</div>
				</div>
			{/if}

			<!-- Activity list -->
			<div>
				<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Activities</div>
				<div class="flex flex-col gap-1.5">
					{#each workflow?.activities ?? [] as act, idx}
						{@const td = getActivityType(act.type)}
						<button
							class="w-full flex items-start gap-2.5 p-2.5 rounded-lg border text-left cursor-pointer transition-colors bg-transparent
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
									<span class="text-xs text-base-content/40 font-mono">{act.steps?.length ?? 0} steps</span>
								</div>
							</div>
						</button>
					{/each}
				</div>
			</div>

			<!-- Delete workflow (edit mode) -->
			{#if isEditable}
				<button
					class="btn btn-sm btn-error btn-outline w-full mt-4"
					onclick={() => onremoveWorkflow?.()}
				>Delete Workflow</button>
			{/if}
		{/if}
	</div>

	<!-- Panel footer -->
	{#if selectedNodeId && activity}
		<div class="px-4 py-3 border-t border-base-content/10 shrink-0">
			<button
				class="text-xs text-primary cursor-pointer bg-transparent border-none hover:underline p-0"
				onclick={() => onselectActivity?.('')}
			>Back to workflow overview</button>
		</div>
	{/if}
</div>
