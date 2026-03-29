<script lang="ts">
	import type { RoleWorkflowEntry as AgentWorkflowEntry, EventSourceOption } from '$lib/api/neboComponents';
	import { createRoleWorkflow as createAgentWorkflow, updateRoleWorkflow as updateAgentWorkflow, listEventSources } from '$lib/api/nebo';
	import { fly } from 'svelte/transition';
	import { Plus, X, GripVertical } from 'lucide-svelte';
	import RichInput from '$lib/components/ui/RichInput.svelte';
	import TagInput from '$lib/components/ui/TagInput.svelte';

	let {
		roleId,
		existing,
		onclose,
		onsave,
	}: {
		roleId: string;
		existing: AgentWorkflowEntry | null;
		onclose: () => void;
		onsave: () => void;
	} = $props();

	const isEdit = $derived(!!existing);

	// Core fields
	let name = $state(existing?.description || '');
	let bindingName = $state(existing?.bindingName || '');
	let bindingNameManual = $state(!!existing);
	let triggerType = $state(existing?.triggerType || 'schedule');
	let saving = $state(false);
	let error = $state('');

	// Schedule fields
	let scheduleHour = $state(7);
	let scheduleMinute = $state(0);
	let scheduleAmPm: 'AM' | 'PM' = $state('AM');
	let scheduleDays = $state<'every' | 'weekdays' | 'weekends' | 'custom'>('every');
	let customDays = $state<boolean[]>([false, false, false, false, false, false, false]);

	// Interval fields
	let intervalValue = $state('30m');
	let intervalWindowEnabled = $state(false);
	let intervalWindowStart = $state('09:00');
	let intervalWindowEnd = $state('18:00');

	// Event fields
	let eventSources = $state<string[]>([]);
	let eventSourceOptions = $state<EventSourceOption[]>([]);
	let eventSourcesLoaded = $state(false);

	// Inputs (key-value rows)
	let inputRows = $state<{ key: string; value: string }[]>([]);

	// Activities (steps) with drag state
	type ActivityRow = { id: string; intent: string };
	let activityRows = $state<ActivityRow[]>([]);
	let dragSrcIdx = $state<number | null>(null);
	let dragOverIdx = $state<number | null>(null);

	// Emit
	let emitEnabled = $state(false);
	let emitName = $state('');
	type EmitPayload = 'output' | 'nothing';
	let emitPayload = $state<EmitPayload>('output');

	// ID editing
	let editingId = $state(false);

	const dayLabels = ['S', 'M', 'T', 'W', 'T', 'F', 'S'];
	const dayNames = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

	const intervalOptions = [
		{ value: '5m', label: '5 minutes' },
		{ value: '10m', label: '10 minutes' },
		{ value: '15m', label: '15 minutes' },
		{ value: '30m', label: '30 minutes' },
		{ value: '1h', label: '1 hour' },
		{ value: '2h', label: '2 hours' },
		{ value: '4h', label: '4 hours' },
		{ value: '8h', label: '8 hours' },
		{ value: '24h', label: '24 hours' },
	];

	const triggerTypes = [
		{ value: 'schedule', label: 'Schedule', icon: '📅' },
		{ value: 'heartbeat', label: 'Interval', icon: '⏱' },
		{ value: 'event', label: 'On Event', icon: '⚡' },
		{ value: 'manual', label: 'Manual', icon: '▶' },
	] as const;

	// Initialize from existing workflow
	if (existing) {
		const cfg = existing.triggerConfig || '';
		switch (existing.triggerType) {
			case 'schedule': {
				const parts = cfg.split(/\s+/);
				if (parts.length >= 5) {
					const min = parseInt(parts[0]);
					const hr = parseInt(parts[1]);
					const dow = parts[4];
					if (!isNaN(min)) scheduleMinute = min;
					if (!isNaN(hr)) {
						if (hr === 0) { scheduleHour = 12; scheduleAmPm = 'AM'; }
						else if (hr < 12) { scheduleHour = hr; scheduleAmPm = 'AM'; }
						else if (hr === 12) { scheduleHour = 12; scheduleAmPm = 'PM'; }
						else { scheduleHour = hr - 12; scheduleAmPm = 'PM'; }
					}
					if (dow === '*') scheduleDays = 'every';
					else if (dow === '1-5') scheduleDays = 'weekdays';
					else if (dow === '0,6') scheduleDays = 'weekends';
					else {
						scheduleDays = 'custom';
						const dayNums = dow.split(',').map(Number);
						customDays = [0, 1, 2, 3, 4, 5, 6].map(d => dayNums.includes(d));
					}
				}
				break;
			}
			case 'heartbeat': {
				const parts = cfg.split('|');
				intervalValue = parts[0] || '30m';
				if (parts[1]) {
					intervalWindowEnabled = true;
					const winParts = parts[1].split('-');
					if (winParts.length === 2) {
						intervalWindowStart = winParts[0];
						intervalWindowEnd = winParts[1];
					}
				}
				break;
			}
			case 'event':
				eventSources = cfg.split(',').map(s => s.trim()).filter(Boolean);
				break;
		}
		let parsedInputs: Record<string, unknown> = {};
		if (existing.inputs) {
			try {
				parsedInputs = typeof existing.inputs === 'string'
					? JSON.parse(existing.inputs)
					: existing.inputs;
				for (const [k, v] of Object.entries(parsedInputs)) {
					if (k !== '_emit' && k !== '_payload') {
						inputRows.push({ key: k, value: typeof v === 'string' ? v : JSON.stringify(v) });
					}
				}
			} catch { /* ignore */ }
		}
		if (existing.emit) {
			emitEnabled = true;
			emitName = existing.emit;
			emitPayload = parsedInputs._payload === 'nothing' ? 'nothing' : 'output';
		}
		if (existing.activities) {
			try {
				const acts = typeof existing.activities === 'string'
					? JSON.parse(existing.activities)
					: existing.activities;
				if (Array.isArray(acts)) {
					activityRows = acts.map((a: any, i: number) => ({
						id: a.id || `step-${i + 1}`,
						intent: a.intent || '',
					}));
				}
			} catch { /* ignore */ }
		}
	}

	// Auto-populate emitName when first enabled
	$effect(() => {
		if (emitEnabled && !emitName && name) {
			emitName = toBindingName(name) + '.done';
		}
	});

	// Lazy-load event source suggestions when trigger type changes to 'event'
	$effect(() => {
		if (triggerType === 'event' && !eventSourcesLoaded) {
			eventSourcesLoaded = true;
			listEventSources().then(res => {
				eventSourceOptions = res.sources || [];
			}).catch(() => {});
		}
	});

	// Close on Escape — but not when inside an editor (let slash menu close first)
	function handleKeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape') return;
		const target = e.target as HTMLElement;
		// If focused inside a Tiptap editor, blur it instead of closing the modal.
		// This lets the slash menu close on first Escape, and a second Escape closes the modal.
		if (target?.closest?.('.ProseMirror')) {
			(target.closest('.ProseMirror') as HTMLElement)?.blur();
			return;
		}
		onclose();
	}

	function toBindingName(n: string): string {
		return n.toLowerCase()
			.replace(/[^a-z0-9\s-]/g, '')
			.trim()
			.replace(/\s+/g, '-')
			.slice(0, 50);
	}

	function handleNameInput(e: Event) {
		name = (e.target as HTMLInputElement).value;
		if (!bindingNameManual) bindingName = toBindingName(name);
	}

	// Cron builder
	function buildCron(): string {
		const hr24 = scheduleAmPm === 'AM'
			? (scheduleHour === 12 ? 0 : scheduleHour)
			: (scheduleHour === 12 ? 12 : scheduleHour + 12);
		let dow = '*';
		if (scheduleDays === 'weekdays') dow = '1-5';
		else if (scheduleDays === 'weekends') dow = '0,6';
		else if (scheduleDays === 'custom') {
			const selected = customDays.map((v, i) => v ? i : -1).filter(i => i >= 0);
			if (selected.length > 0 && selected.length < 7) dow = selected.join(',');
		}
		return `${scheduleMinute} ${hr24} * * ${dow}`;
	}

	const cronPreview = $derived.by(() => {
		if (triggerType !== 'schedule') return '';
		return buildCron();
	});

	const emitPreview = $derived.by(() => {
		if (!emitEnabled) return '';
		const agentSlug = toBindingName(name || 'agent');
		const eventName = emitName.trim() || (agentSlug + '.done');
		if (emitPayload === 'nothing') {
			return JSON.stringify({
				source: `${agentSlug}.${eventName}`,
				agent: name || 'Agent',
				timestamp: '<unix timestamp>'
			}, null, 2);
		}
		return JSON.stringify({
			source: `${agentSlug}.${eventName}`,
			output: "<the agent's result>",
			agent: name || 'Agent',
			timestamp: '<unix timestamp>'
		}, null, 2);
	});

	function buildTriggerConfig(): Record<string, unknown> {
		switch (triggerType) {
			case 'schedule': return { cron: buildCron() };
			case 'heartbeat': {
				const cfg: Record<string, unknown> = { interval: intervalValue };
				if (intervalWindowEnabled) cfg.window = `${intervalWindowStart}-${intervalWindowEnd}`;
				return cfg;
			}
			case 'event': return { sources: eventSources.join(',') };
			default: return {};
		}
	}

	// Input rows
	function addInputRow() { inputRows = [...inputRows, { key: '', value: '' }]; }
	function removeInputRow(idx: number) { inputRows = inputRows.filter((_, i) => i !== idx); }

	// Activity rows
	function addActivityRow() {
		activityRows = [...activityRows, { id: `step-${activityRows.length + 1}`, intent: '' }];
	}
	function removeActivityRow(idx: number) {
		activityRows = activityRows
			.filter((_, i) => i !== idx)
			.map((a, i) => ({ ...a, id: `step-${i + 1}` }));
	}
	function renumberActivities(rows: ActivityRow[]): ActivityRow[] {
		return rows.map((a, i) => ({ ...a, id: `step-${i + 1}` }));
	}

	// Drag and drop
	function onDragStart(e: DragEvent, idx: number) {
		dragSrcIdx = idx;
		if (e.dataTransfer) {
			e.dataTransfer.effectAllowed = 'move';
			e.dataTransfer.setData('text/plain', String(idx));
		}
	}
	function onDragOver(e: DragEvent, idx: number) {
		e.preventDefault();
		if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
		dragOverIdx = idx;
	}
	function onDrop(e: DragEvent, idx: number) {
		e.preventDefault();
		if (dragSrcIdx === null || dragSrcIdx === idx) {
			dragSrcIdx = null;
			dragOverIdx = null;
			return;
		}
		const rows = [...activityRows];
		const [moved] = rows.splice(dragSrcIdx, 1);
		rows.splice(idx, 0, moved);
		activityRows = renumberActivities(rows);
		dragSrcIdx = null;
		dragOverIdx = null;
	}
	function onDragEnd() {
		dragSrcIdx = null;
		dragOverIdx = null;
	}

	// Extract {{type:id:name}} refs from a step intent string
	function extractRefs(intent: string) {
		const refs: { type: string; id: string; name: string }[] = [];
		const re = /\{\{(mcp|skill|agent|cmd):([^:]+):([^}]+)\}\}/g;
		let m;
		while ((m = re.exec(intent)) !== null) {
			refs.push({ type: m[1], id: m[2], name: m[3] });
		}
		return refs;
	}

	async function handleSave() {
		const bn = bindingName.trim();
		if (!bn) { error = 'Name is required'; return; }
		const validActivities = activityRows.filter(a => a.intent.trim());
		if (validActivities.length === 0) {
			error = 'Add at least one step — what should this automation do?';
			return;
		}
		error = '';
		saving = true;
		try {
			const inputs: Record<string, unknown> = {};
			for (const row of inputRows) {
				if (row.key.trim()) inputs[row.key.trim()] = row.value;
			}
			if (emitEnabled && emitPayload !== 'output') inputs['_payload'] = emitPayload;

			const activities = validActivities.map((a, i) => {
				const refs = extractRefs(a.intent);
				const skills = refs.filter(r => r.type === 'skill').map(r => r.id);
				const mcps = refs.filter(r => r.type === 'mcp').map(r => r.id);
				const cmds = refs.filter(r => r.type === 'cmd').map(r => r.id);
				const cleanIntent = a.intent.replace(/\{\{[^}]+\}\}/g, (m) => {
					const parts = m.slice(2, -2).split(':');
					return parts[2] || parts[1];
				});
				return {
					id: a.id || `step-${i + 1}`,
					intent: cleanIntent.trim(),
					skills: skills.length > 0 ? skills : undefined,
					mcps: mcps.length > 0 ? mcps : undefined,
					cmds: cmds.length > 0 ? cmds : undefined,
				};
			});

			const payload: Record<string, unknown> = {
				bindingName: bn,
				triggerType,
				triggerConfig: buildTriggerConfig(),
				description: name || undefined,
				inputs: Object.keys(inputs).length > 0 ? inputs : undefined,
				emit: emitEnabled && emitName.trim() ? emitName.trim() : null,
				activities: activities.length > 0 ? activities : undefined,
			};

			if (isEdit && existing) {
				await updateAgentWorkflow(roleId, existing.bindingName, payload);
			} else {
				await createAgentWorkflow(roleId, payload);
			}
			onsave();
		} catch (e: any) {
			error = e?.error || e?.message || 'Failed to save';
		} finally {
			saving = false;
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- Full-page modal overlay -->
<div
	class="fixed inset-0 z-[60] flex flex-col bg-base-100"
	transition:fly={{ y: 40, duration: 250 }}
>
		<!-- Header -->
		<div class="flex items-center justify-between px-6 py-4 border-b border-base-content/10 shrink-0">
			<h2 class="font-semibold text-base text-base-content">
				{isEdit ? 'Edit Automation' : 'New Automation'}
			</h2>
			<div class="flex items-center gap-2">
				<button
					type="button"
					class="btn btn-ghost btn-sm"
					onclick={onclose}
				>
					Cancel
				</button>
				<button
					type="button"
					class="btn btn-primary btn-sm"
					disabled={saving || !bindingName.trim()}
					onclick={handleSave}
				>
					{#if saving}
						<span class="loading loading-spinner loading-xs"></span>
					{/if}
					{isEdit ? 'Save' : 'Create'}
				</button>
			</div>
		</div>

		<!-- Scrollable body -->
		<div class="flex-1 overflow-y-auto">
			<div class="max-w-2xl mx-auto w-full px-6 py-6 flex flex-col gap-7">

				{#if error}
					<div class="text-sm text-error bg-error/10 rounded-lg px-4 py-3">{error}</div>
				{/if}

				<!-- Name -->
				<div>
					<label class="text-sm font-medium text-base-content/80 block mb-1.5" for="auto-name">Name</label>
					<input
						id="auto-name"
						type="text"
						class="w-full h-10 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
						placeholder="Morning Briefing"
						value={name}
						oninput={handleNameInput}
					/>
				</div>

				<!-- Steps -->
				<div>
					<div class="flex items-center justify-between mb-2">
						<div>
							<span class="text-sm font-medium text-base-content/80">Steps</span>
							<p class="text-xs text-base-content/40 mt-0.5">Run in order. Each step sees what previous ones produced.</p>
						</div>
						{#if activityRows.length > 0}
							<button type="button" class="flex items-center gap-1 text-xs text-primary hover:text-primary/80 transition-colors" onclick={addActivityRow}>
								<Plus class="w-3 h-3" /> Add step
							</button>
						{/if}
					</div>

					{#if activityRows.length === 0}
						<button
							type="button"
							class="w-full rounded-xl border-2 border-dashed border-base-content/15 px-4 py-6 text-sm text-base-content/40 hover:border-primary/30 hover:text-primary/60 transition-colors text-center"
							onclick={addActivityRow}
						>
							<Plus class="w-4 h-4 mx-auto mb-1.5 opacity-50" />
							Add a step — what should the agent do?
						</button>
					{:else}
						<div class="flex flex-col">
							{#each activityRows as row, i (row.id)}
								<div
									class="relative"
									draggable="true"
									ondragstart={(e) => onDragStart(e, i)}
									ondragover={(e) => onDragOver(e, i)}
									ondrop={(e) => onDrop(e, i)}
									ondragend={onDragEnd}
								>
									{#if dragOverIdx === i && dragSrcIdx !== i}
										<div class="absolute -top-0.5 left-10 right-0 h-0.5 bg-primary rounded-full z-10"></div>
									{/if}

									<div class="flex items-start gap-2.5 py-1.5 group" class:opacity-40={dragSrcIdx === i}>
										<!-- Drag handle -->
										<div class="pt-3 shrink-0 cursor-grab active:cursor-grabbing">
											<GripVertical class="w-4 h-4 text-base-content/20 group-hover:text-base-content/40 transition-colors" />
										</div>
										<!-- Step number -->
										<div class="w-6 h-6 rounded-full bg-base-content/8 flex items-center justify-center text-xs font-mono text-base-content/40 shrink-0 mt-2.5">
											{i + 1}
										</div>
										<!-- Step input with / mentions -->
										<div class="flex-1">
											<RichInput
												bind:value={row.intent}
												currentRoleId={roleId}
												mode="minimal"
												placeholder={i === 0
													? 'e.g. Gather top tech news · type / to mention an MCP or skill'
													: i === activityRows.length - 1
														? 'e.g. Send a summary as a chat message'
														: 'e.g. Summarize the key findings into bullet points'}
											/>
										</div>

										<!-- Remove -->
										<button
											type="button"
											class="btn btn-xs btn-ghost btn-square text-base-content/20 hover:text-error/70 mt-2.5 shrink-0 opacity-0 group-hover:opacity-100 transition-opacity"
											onclick={() => removeActivityRow(i)}
											title="Remove step"
										>
											<X class="w-3.5 h-3.5" />
										</button>
									</div>

									<!-- Connector -->
									{#if i < activityRows.length - 1}
										<div class="flex items-center gap-2 pl-[52px] py-0.5">
											<div class="flex flex-col items-center">
												<div class="w-px h-2.5 bg-base-content/15"></div>
												<svg width="8" height="5" viewBox="0 0 8 5" fill="currentColor" class="text-base-content/20">
													<path d="M4 5L0 0h8L4 5z"/>
												</svg>
											</div>
											<span class="text-xs text-base-content/25">passes result to step {i + 2}</span>
										</div>
									{/if}
								</div>
							{/each}

							<div class="pl-[52px] pt-2">
								<button type="button" class="flex items-center gap-1.5 text-xs text-base-content/35 hover:text-primary/60 transition-colors" onclick={addActivityRow}>
									<Plus class="w-3 h-3" /> Add another step
								</button>
							</div>
						</div>
					{/if}
				</div>

				<div class="border-t border-base-content/10"></div>

				<!-- Trigger type -->
				<div>
					<label class="text-sm font-medium text-base-content/80 block mb-2.5">When should it run?</label>
					<div class="grid grid-cols-4 gap-2">
						{#each triggerTypes as tt}
							<button
								type="button"
								class="flex flex-col items-center gap-1.5 px-3 py-3 rounded-xl border text-sm transition-colors
									{triggerType === tt.value
										? 'border-primary/30 bg-primary/10 text-primary'
										: 'border-base-content/10 bg-base-content/5 text-base-content/70 hover:bg-base-content/10'}"
								onclick={() => triggerType = tt.value}
							>
								<span class="text-xl">{tt.icon}</span>
								<span class="font-medium text-xs">{tt.label}</span>
							</button>
						{/each}
					</div>

					<div class="mt-4 flex flex-col gap-3">
						{#if triggerType === 'schedule'}
							<div class="flex items-center gap-3">
								<span class="text-sm text-base-content/60 w-10 shrink-0">Time</span>
								<div class="flex items-center gap-1.5">
									<select class="h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-2 text-sm focus:outline-none focus:border-primary/50" bind:value={scheduleHour}>
										{#each Array.from({ length: 12 }, (_, i) => i + 1) as h}
											<option value={h}>{h}</option>
										{/each}
									</select>
									<span class="text-base-content/30">:</span>
									<select class="h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-2 text-sm focus:outline-none focus:border-primary/50" bind:value={scheduleMinute}>
										{#each [0, 15, 30, 45] as m}
											<option value={m}>{m.toString().padStart(2, '0')}</option>
										{/each}
									</select>
									<select class="h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-2 text-sm focus:outline-none focus:border-primary/50" bind:value={scheduleAmPm}>
										<option value="AM">AM</option>
										<option value="PM">PM</option>
									</select>
								</div>
							</div>
							<div class="flex items-center gap-3">
								<span class="text-sm text-base-content/60 w-10 shrink-0">Days</span>
								<select class="h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 flex-1" bind:value={scheduleDays}>
									<option value="every">Every day</option>
									<option value="weekdays">Weekdays (Mon-Fri)</option>
									<option value="weekends">Weekends</option>
									<option value="custom">Custom...</option>
								</select>
							</div>
							{#if scheduleDays === 'custom'}
								<div class="flex gap-1.5 pl-[52px]">
									{#each dayLabels as day, i}
										<button type="button"
											class="w-8 h-8 rounded-lg text-xs font-medium transition-colors {customDays[i] ? 'bg-primary text-primary-content' : 'bg-base-content/5 text-base-content/60 hover:bg-base-content/10'}"
											onclick={() => { customDays[i] = !customDays[i]; customDays = [...customDays]; }}
											title={dayNames[i]}>{day}</button>
									{/each}
								</div>
							{/if}
							<p class="text-xs text-base-content/30 pl-[52px] font-mono">&#8627; {cronPreview}</p>

						{:else if triggerType === 'heartbeat'}
							<div class="flex items-center gap-3">
								<span class="text-sm text-base-content/60 w-10 shrink-0">Every</span>
								<select class="h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 flex-1" bind:value={intervalValue}>
									{#each intervalOptions as opt}
										<option value={opt.value}>{opt.label}</option>
									{/each}
								</select>
							</div>
							<div class="flex items-center gap-3">
								<label class="flex items-center gap-2 cursor-pointer">
									<input type="checkbox" class="checkbox checkbox-sm checkbox-primary" bind:checked={intervalWindowEnabled} />
									<span class="text-sm text-base-content/60">Limit to hours</span>
								</label>
								{#if intervalWindowEnabled}
									<div class="flex items-center gap-2 flex-1">
										<input type="time" class="h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-2 text-sm focus:outline-none focus:border-primary/50" bind:value={intervalWindowStart} />
										<span class="text-sm text-base-content/30">to</span>
										<input type="time" class="h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-2 text-sm focus:outline-none focus:border-primary/50" bind:value={intervalWindowEnd} />
									</div>
								{/if}
							</div>

						{:else if triggerType === 'event'}
							<div>
								<label class="text-sm text-base-content/60 block mb-1.5">Event sources</label>
								<TagInput bind:value={eventSources} placeholder="Type an event name and press Enter..." />
								{#if eventSourceOptions.length > 0}
									{@const available = eventSourceOptions.filter(o => !eventSources.includes(o.value))}
									{#if available.length > 0}
										<div class="flex flex-wrap gap-1.5 mt-2">
											{#each available as opt}
												<button
													type="button"
													class="inline-flex items-center gap-1 px-2.5 py-1 rounded-full text-xs bg-base-content/5 border border-base-content/10 text-base-content/60 hover:border-primary/30 hover:text-primary transition-colors"
													onclick={() => { eventSources = [...eventSources, opt.value]; }}
													title={opt.description || `From ${opt.roleName}`}
												>
													<Plus class="w-3 h-3" />
													{opt.value}
												</button>
											{/each}
										</div>
									{/if}
								{/if}
								<p class="text-xs text-base-content/35 mt-1.5">Pick from available events or type custom ones. Wildcards like <code class="font-mono">email.*</code> also work.</p>
								{#if eventSources.length > 0}
									<div class="mt-3 rounded-lg bg-base-content/5 border border-base-content/10 px-3 py-2.5">
										<p class="text-xs text-base-content/40 mb-1.5 uppercase tracking-wider font-medium">What your agent receives</p>
										<pre class="text-xs text-base-content/60 font-mono leading-relaxed whitespace-pre-wrap">{JSON.stringify({ _event_source: eventSources.join(', '), _event_payload: "<the announcing agent's output>", _event_origin: "<run id>" }, null, 2)}</pre>
									</div>
								{/if}
							</div>

						{:else}
							<p class="text-sm text-base-content/50">Run only when you ask for it.</p>
						{/if}
					</div>
				</div>

				<div class="border-t border-base-content/10"></div>

				<!-- Needs information -->
				<div>
					<div class="flex items-center justify-between mb-2">
						<div>
							<span class="text-sm font-medium text-base-content/80">Needs information</span>
							<p class="text-xs text-base-content/40 mt-0.5">Default values passed to this automation.</p>
						</div>
						<button type="button" class="flex items-center gap-1 text-xs text-primary hover:text-primary/80 transition-colors" onclick={addInputRow}>
							<Plus class="w-3 h-3" /> Add field
						</button>
					</div>
					{#if inputRows.length === 0}
						<p class="text-xs text-base-content/35">No fields yet.</p>
					{:else}
						<div class="flex flex-col gap-2">
							{#each inputRows as row, i}
								<div class="flex items-center gap-2">
									<input type="text" class="w-2/5 h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors" placeholder="Field name" bind:value={row.key} />
									<input type="text" class="flex-1 h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors" placeholder="Default value" bind:value={row.value} />
									<button type="button" class="btn btn-xs btn-ghost btn-square text-base-content/40 hover:text-error/80" onclick={() => removeInputRow(i)}><X class="w-3.5 h-3.5" /></button>
								</div>
							{/each}
						</div>
					{/if}
				</div>

				<div class="border-t border-base-content/10"></div>

				<!-- When done, announce -->
				<div>
					<label class="flex items-center gap-2 cursor-pointer mb-3">
						<input type="checkbox" class="checkbox checkbox-sm checkbox-primary" bind:checked={emitEnabled} />
						<span class="text-sm font-medium text-base-content/80">When done, announce</span>
					</label>
					{#if emitEnabled}
						<div class="pl-6 flex flex-col gap-4">
							<div>
								<label class="text-xs text-base-content/50 block mb-1" for="emit-name">Event name</label>
								<input id="emit-name" type="text" class="w-full h-9 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm font-mono focus:outline-none focus:border-primary/50 transition-colors" placeholder="{toBindingName(name || 'automation')}.done" bind:value={emitName} />
								<p class="text-xs text-base-content/35 mt-1">Other automations can listen for this event.</p>
							</div>
							<div>
								<span class="text-xs text-base-content/50 block mb-2">What to include</span>
								<div class="flex flex-col gap-2">
									<label class="flex items-start gap-2.5 cursor-pointer">
										<input type="radio" class="radio radio-xs radio-primary mt-0.5" name="emit-payload" value="output" bind:group={emitPayload} />
										<div>
											<span class="text-sm text-base-content/80 font-medium">My output</span>
											<p class="text-xs text-base-content/45 mt-0.5">The result from the last step</p>
										</div>
									</label>
									<label class="flex items-start gap-2.5 cursor-pointer">
										<input type="radio" class="radio radio-xs radio-primary mt-0.5" name="emit-payload" value="nothing" bind:group={emitPayload} />
										<div>
											<span class="text-sm text-base-content/80 font-medium">Nothing</span>
											<p class="text-xs text-base-content/45 mt-0.5">Signal only</p>
										</div>
									</label>
								</div>
							</div>
							{#if emitPreview}
								<div class="rounded-lg bg-base-content/5 border border-base-content/10 px-3 py-2.5">
									<p class="text-xs text-base-content/40 mb-1.5 uppercase tracking-wider font-medium">What gets sent</p>
									<pre class="text-xs text-base-content/60 font-mono leading-relaxed whitespace-pre-wrap">{emitPreview}</pre>
								</div>
							{/if}
						</div>
					{/if}
				</div>

				<!-- ID -->
				<div class="pb-4">
					{#if editingId && !isEdit}
						<div class="flex items-center gap-2">
							<span class="text-xs text-base-content/35">ID:</span>
							<input type="text" class="flex-1 h-7 rounded bg-base-content/5 border border-base-content/10 px-2 text-xs font-mono text-base-content/50 focus:outline-none focus:border-primary/50" bind:value={bindingName} oninput={() => { bindingNameManual = true; }} />
							<button type="button" class="text-xs text-base-content/40 hover:text-base-content/60" onclick={() => editingId = false}>done</button>
						</div>
					{:else}
						<button type="button" class="text-xs text-base-content/30 hover:text-base-content/50 transition-colors font-mono" onclick={() => { if (!isEdit) editingId = true; }} disabled={isEdit}>
							ID: {bindingName || '(auto)'}
						</button>
					{/if}
				</div>

			</div>
		</div>
	</div>

