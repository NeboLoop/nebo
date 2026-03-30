<script lang="ts">
	import { installStoreProduct, listAgents, activateAgent, getAgent, updateAgentInputs, getAgentWorkflows, updateAgentWorkflow } from '$lib/api/nebo';
	import type { AgentInputField, AgentWorkflowEntry } from '$lib/api/neboComponents';
	import AgentInputForm from '$lib/components/agent/AgentInputForm.svelte';
	import { X } from 'lucide-svelte';

	let {
		appId,
		agentName,
		agentDescription,
		inputs = {},
		onComplete,
		onCancel,
	}: {
		appId: string;
		agentName: string;
		agentDescription: string;
		inputs: Record<string, unknown>;
		onComplete: (agentId: string) => void;
		onCancel: () => void;
	} = $props();

	// Wizard steps
	type Step = 'inputs' | 'schedule' | 'installing' | 'done';
	let step = $state<Step>('inputs');
	let error = $state('');

	// Agent data (loaded after install)
	let agentId = $state('');
	let inputFields = $state<AgentInputField[]>([]);
	let inputValues = $state<Record<string, unknown>>({});
	let workflows = $state<AgentWorkflowEntry[]>([]);

	// Schedule overrides (binding name → user-chosen interval label)
	let scheduleOverrides = $state<Record<string, string>>({});

	// If inputs is an array of field definitions, normalize to AgentInputField format
	if (Array.isArray(inputs)) {
		inputFields = inputs.map((f: any) => ({
			key: f.key || f.name || '',
			label: f.label || (f.name || '').replace(/[_-]/g, ' ').replace(/\b\w/g, (c: string) => c.toUpperCase()),
			description: f.description || '',
			type: f.type || 'text',
			required: f.required || false,
			default: f.default,
			placeholder: f.placeholder || '',
			options: Array.isArray(f.options) ? f.options.map((o: any) =>
			typeof o === 'string' ? { value: o, label: o.replace(/[_-]/g, ' ').replace(/\b\w/g, (c: string) => c.toUpperCase()) } : o
		) : f.options,
		}));
	} else {
		// Legacy fallback: if old-style flat inputs, convert to values
		for (const [key, val] of Object.entries(inputs)) {
			inputValues[key] = val;
		}
	}

	const hasInputFields = $derived(inputFields.length > 0);
	const hasSchedules = $derived(workflows.some(w =>
		w.isActive && (w.triggerType === 'schedule' || w.triggerType === 'heartbeat')
	));

	const intervalOptions = [
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

	function summarizeTrigger(wf: AgentWorkflowEntry): string {
		if (wf.triggerType === 'heartbeat') {
			const interval = wf.triggerConfig.split('|')[0] || '30m';
			const match = intervalOptions.find(o => o.value === interval);
			return match?.label || `Every ${interval}`;
		}
		if (wf.triggerType === 'schedule') {
			return `Scheduled: ${wf.triggerConfig}`;
		}
		return wf.triggerType;
	}

	async function handleInstall() {
		step = 'installing';
		error = '';
		try {
			// 1. Install
			await installStoreProduct(appId);

			// 2. Find the agent
			const agentsRes = await listAgents();
			const allAgents = agentsRes?.agents || [];
			const matchedAgent = allAgents.find(
				(r: any) => r.name?.toLowerCase() === agentName.toLowerCase()
			);

			if (!matchedAgent) {
				error = 'Agent installed but could not be found.';
				step = 'inputs';
				return;
			}

			agentId = matchedAgent.id;

			// 3. Load normalized input fields from backend
			try {
				const agentRes = await getAgent(agentId);
				if (agentRes?.inputFields) {
					inputFields = agentRes.inputFields;
				}
			} catch { /* ignore */ }

			// 4. Load workflows for schedule config
			try {
				const wfRes = await getAgentWorkflows(agentId);
				workflows = wfRes?.workflows || [];
			} catch { /* ignore */ }

			// 5. Save input values
			if (Object.keys(inputValues).length > 0) {
				await updateAgentInputs(agentId, inputValues).catch(() => {});
			}

			// If there are schedules to configure, go to schedule step
			if (hasSchedules) {
				step = 'schedule';
			} else {
				await finalize();
			}
		} catch (e: any) {
			error = e?.error || e?.message || 'Failed to install agent';
			step = 'inputs';
		}
	}

	async function handleScheduleDone() {
		step = 'installing';
		try {
			// Apply schedule overrides
			for (const [bindingName, interval] of Object.entries(scheduleOverrides)) {
				const wf = workflows.find(w => w.bindingName === bindingName);
				if (!wf) continue;

				if (wf.triggerType === 'heartbeat') {
					const parts = wf.triggerConfig.split('|');
					const newConfig = parts.length > 1 ? `${interval}|${parts[1]}` : interval;
					await updateAgentWorkflow(agentId, bindingName, {
						triggerType: 'heartbeat',
						triggerConfig: { interval, ...(parts[1] ? { window: parts[1] } : {}) },
					}).catch(() => {});
				}
			}
			await finalize();
		} catch (e: any) {
			error = e?.error || e?.message || 'Failed to configure schedules';
			step = 'schedule';
		}
	}

	async function finalize() {
		await activateAgent(agentId);
		step = 'done';
		setTimeout(() => onComplete(agentId), 800);
	}
</script>

<div class="fixed inset-0 z-[60] flex items-center justify-center p-4 sm:p-8">
	<div class="absolute inset-0 bg-black/60 backdrop-blur-sm"></div>

	<div class="relative w-full max-w-lg rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden max-h-[90vh] flex flex-col">
		{#if step === 'installing'}
			<div class="flex flex-col items-center justify-center py-16 px-6">
				<span class="loading loading-spinner loading-lg text-primary"></span>
				<p class="text-base font-medium mt-4">Setting up {agentName}...</p>
				<p class="text-sm text-base-content/70 mt-1">This just takes a moment</p>
			</div>

		{:else if step === 'done'}
			<div class="flex flex-col items-center justify-center py-16 px-6">
				<svg class="w-12 h-12 text-success" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" /><polyline points="22 4 12 14.01 9 11.01" />
				</svg>
				<p class="text-base font-medium mt-4">{agentName} is ready!</p>
				<p class="text-sm text-base-content/70 mt-1">Your agent is now active and working.</p>
			</div>

		{:else if step === 'schedule'}
			<!-- Schedule configuration -->
			<div class="flex items-center justify-between px-6 pt-6 pb-2">
				<div></div>
				<button
					type="button"
					class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors"
					onclick={onCancel}
					aria-label="Close"
				>
					<X class="w-4 h-4 text-base-content/70" />
				</button>
			</div>

			<div class="px-6 pb-6 overflow-y-auto">
				<div class="text-center mb-6">
					<h2 class="font-display text-xl font-bold">How often should it run?</h2>
					<p class="text-sm text-base-content/70 mt-1">You can change these anytime in the Automate tab.</p>
				</div>

				{#if error}
					<div class="text-sm text-error bg-error/10 rounded-lg px-3 py-2 mb-4">{error}</div>
				{/if}

				<div class="flex flex-col gap-4 mb-6">
					{#each workflows.filter(w => w.isActive && (w.triggerType === 'schedule' || w.triggerType === 'heartbeat')) as wf}
						<div class="rounded-xl border border-base-content/10 p-4">
							<p class="text-sm font-medium mb-1">{wf.description || wf.bindingName}</p>
							<p class="text-xs text-base-content/70 mb-3">Currently: {summarizeTrigger(wf)}</p>

							{#if wf.triggerType === 'heartbeat'}
								<select
									class="select select-bordered select-sm w-full"
									value={scheduleOverrides[wf.bindingName] || wf.triggerConfig.split('|')[0] || '30m'}
									onchange={(e) => scheduleOverrides[wf.bindingName] = (e.target as HTMLSelectElement).value}
								>
									{#each intervalOptions as opt}
										<option value={opt.value}>{opt.label}</option>
									{/each}
								</select>
							{:else}
								<p class="text-xs text-base-content/70">This runs on a fixed schedule.</p>
							{/if}
						</div>
					{/each}
				</div>

				<div class="flex gap-3">
					<button
						type="button"
						class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
						onclick={() => { step = 'inputs'; }}
					>
						Back
					</button>
					<button
						type="button"
						class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
						onclick={handleScheduleDone}
					>
						Start working
					</button>
				</div>
			</div>

		{:else}
			<!-- Step 1: Inputs -->
			<div class="flex items-center justify-between px-6 pt-6 pb-2">
				<div></div>
				<button
					type="button"
					class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors"
					onclick={onCancel}
					aria-label="Close"
				>
					<X class="w-4 h-4 text-base-content/70" />
				</button>
			</div>

			<div class="px-6 pb-6 overflow-y-auto">
				<div class="text-center mb-6">
					<h2 class="font-display text-xl font-bold">Set up {agentName}</h2>
					{#if agentDescription}
						<p class="text-sm text-base-content/70 mt-1 line-clamp-2">{agentDescription}</p>
					{/if}
				</div>

				{#if error}
					<div class="text-sm text-error bg-error/10 rounded-lg px-3 py-2 mb-4">{error}</div>
				{/if}

				{#if inputFields.length > 0}
					<div class="border-t border-base-content/10 pt-4 mb-6">
						<p class="text-sm text-base-content/70 mb-4">
							Before {agentName} gets to work, tell it a bit about you.
						</p>
						<AgentInputForm
							fields={inputFields}
							bind:values={inputValues}
							onchange={(v) => inputValues = v}
						/>
					</div>
				{:else if Object.keys(inputs).length > 0}
					<!-- Legacy fallback: flat key-value inputs -->
					<div class="border-t border-base-content/10 pt-4 mb-6">
						<p class="text-sm text-base-content/70 mb-4">
							Before {agentName} gets to work, tell it a bit about you.
						</p>
						<div class="flex flex-col gap-4">
							{#each Object.keys(inputs) as key}
								<div>
									<label class="text-sm font-medium text-base-content/80 block mb-1.5" for="setup-{key}">
										{key.replace(/[_-]/g, ' ').replace(/\b\w/g, c => c.toUpperCase())}
									</label>
									<input
										id="setup-{key}"
										type="text"
										class="input input-bordered w-full text-sm"
										value={inputValues[key] != null ? String(inputValues[key]) : ''}
										oninput={(e) => inputValues = { ...inputValues, [key]: (e.target as HTMLInputElement).value }}
									/>
								</div>
							{/each}
						</div>
					</div>
				{:else}
					<p class="text-sm text-base-content/70 text-center mb-6">
						No configuration needed — {agentName} is ready to go!
					</p>
				{/if}

				<div class="flex gap-3">
					<button
						type="button"
						class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
						onclick={onCancel}
					>
						Cancel
					</button>
					<button
						type="button"
						class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
						onclick={handleInstall}
					>
						{hasInputFields || Object.keys(inputs).length > 0 ? 'Next' : 'Install & Start'}
					</button>
				</div>
			</div>
		{/if}
	</div>
</div>
