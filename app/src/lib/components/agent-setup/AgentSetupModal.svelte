<script lang="ts">
	import { installStoreProduct, activateAgent, getAgent, updateAgentInputs, listAgentWorkflows, updateAgentWorkflow, authLogin } from '$lib/api/nebo';
	import type { AgentWorkflow } from '$lib/api/neboComponents';
	import type { AgentInputField } from '$lib/types/agentPage';
	import AgentInputForm from '$lib/components/agent/AgentInputForm.svelte';
	import { X, KeyRound } from 'lucide-svelte';
	import { onMount, onDestroy } from 'svelte';

	let {
		appId,
		agentName,
		agentDescription,
		inputs = {},
		dependencies = undefined,
		existingAgentId,
		onComplete,
		onCancel,
		onUninstall,
	}: {
		appId: string;
		agentName: string;
		agentDescription: string;
		inputs: Record<string, unknown> | Record<string, unknown>[];
		/** Marketplace product dependencies: { agents?, skills?, plugins?, workflows? } */
		dependencies?: unknown;
		existingAgentId?: string;
		onComplete: (agentId: string) => void;
		onCancel: () => void;
		/** Configure mode only (existingAgentId set): remove the installed agent. */
		onUninstall?: () => void;
	} = $props();

	// Configure mode = reconfiguring an already-installed agent (no fresh install).
	const configuring = $derived(Boolean(existingAgentId));

	type Step = 'inputs' | 'auth' | 'schedule' | 'installing' | 'installing-deps' | 'done';
	let step = $state<Step>('inputs');
	let error = $state('');

	let agentId = $state('');
	let inputFields = $state<AgentInputField[]>([]);
	let inputValues = $state<Record<string, unknown>>({});
	let workflows = $state<AgentWorkflow[]>([]);

	let scheduleOverrides = $state<Record<string, string>>({});

	// Plugin auth state
	interface PluginAuthEntry { slug: string; label: string; description: string; }
	let authQueue = $state<PluginAuthEntry[]>([]);
	let authIndex = $state(0);
	let authInProgress = $state(false);

	const currentAuthPlugin = $derived(authQueue[authIndex]);

	// Dependency install progress
	type DepUiState = 'pending' | 'installing' | 'done' | 'failed';
	interface DepRow { reference: string; depType: string; label: string; state: DepUiState; error?: string; }
	let depRows = $state<DepRow[]>([]);
	let depsAdvanced = false;
	let depTimeout: ReturnType<typeof setTimeout> | undefined;
	const installedDeps = $derived(depRows.filter(d => d.state === 'done').length);

	/** Last segment of a qualified ref, version-stripped: `@org/type/name@1.0` → `name`. */
	function prettyRef(ref: string): string {
		let s = ref;
		const at = s.indexOf('@', 1);
		if (at > 0) s = s.slice(0, at);
		return s.split('/').pop() || ref;
	}

	/** Build the initial ring list from the marketplace product `dependencies` object. */
	function normalizeDeps(): DepRow[] {
		const out: DepRow[] = [];
		const dep = dependencies as any;
		if (!dep || typeof dep !== 'object') return out;
		for (const [key, type] of [['agents', 'agent'], ['skills', 'skill'], ['plugins', 'plugin'], ['workflows', 'workflow']] as const) {
			const arr = dep[key];
			if (!Array.isArray(arr)) continue;
			for (const item of arr) {
				const reference = typeof item === 'string' ? item : (item?.qualifiedName || item?.id || '');
				if (!reference) continue;
				const label = (item && typeof item === 'object' && item.name) ? item.name : prettyRef(reference);
				out.push({ reference, depType: type, label, state: 'pending' });
			}
		}
		return out;
	}

	function ensureDepRow(reference: string, depTypeRaw?: string): DepRow {
		let idx = depRows.findIndex(d => d.reference === reference || prettyRef(d.reference) === prettyRef(reference));
		if (idx === -1) {
			depRows = [...depRows, { reference, depType: (depTypeRaw || '').toLowerCase() || 'skill', label: prettyRef(reference), state: 'pending' }];
			idx = depRows.length - 1;
		}
		return depRows[idx];
	}

	function handleDepStarted(e: CustomEvent) {
		const row = ensureDepRow(e.detail?.reference, e.detail?.depType);
		if (row.state !== 'done' && row.state !== 'failed') row.state = 'installing';
	}
	function handleDepInstalled(e: CustomEvent) {
		ensureDepRow(e.detail?.reference, e.detail?.depType).state = 'done';
	}
	function handleDepFailed(e: CustomEvent) {
		const row = ensureDepRow(e.detail?.reference, e.detail?.depType);
		row.state = 'failed';
		row.error = e.detail?.error;
	}
	function handleDepPending(e: CustomEvent) {
		ensureDepRow(e.detail?.reference, e.detail?.depType);
	}
	function handleDepCascadeComplete() {
		if (step !== 'installing-deps') return;
		// The backend force-installs declared deps at install time, so the cascade
		// always settles via dep_* events — there is no "pending" to approve.
		finishDeps();
	}

	function finishDeps() {
		// Already-installed deps emit no event — resolve any leftovers as done.
		depRows = depRows.map(d => (d.state === 'pending' || d.state === 'installing') ? { ...d, state: 'done' } : d);
		if (depsAdvanced) return;
		depsAdvanced = true;
		afterDepsInstalled();
	}

	$effect(() => {
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
			for (const [key, val] of Object.entries(inputs)) {
				inputValues[key] = val;
			}
		}
	});

	const hasInputFields = $derived(inputFields.length > 0);
	const hasSchedules = $derived(Array.isArray(workflows) && workflows.some(w =>
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

	// WS event listeners for auth flow
	function handleAuthComplete(e: CustomEvent) {
		const data = e.detail;
		if (!currentAuthPlugin || data?.plugin !== currentAuthPlugin.slug) return;
		authInProgress = false;
		advanceAuth();
	}

	function handleAuthError(e: CustomEvent) {
		const data = e.detail;
		if (!currentAuthPlugin || data?.plugin !== currentAuthPlugin.slug) return;
		authInProgress = false;
		error = data?.error || 'Authentication failed';
	}

	function handleAuthUrl(e: CustomEvent) {
		const data = e.detail;
		if (data?.url) {
			window.open(data.url, '_blank');
		}
	}

	onMount(() => {
		window.addEventListener('nebo:plugin_auth_complete', handleAuthComplete as EventListener);
		window.addEventListener('nebo:plugin_auth_error', handleAuthError as EventListener);
		window.addEventListener('nebo:plugin_auth_url', handleAuthUrl as EventListener);
		window.addEventListener('nebo:dep_started', handleDepStarted as EventListener);
		window.addEventListener('nebo:dep_installed', handleDepInstalled as EventListener);
		window.addEventListener('nebo:dep_failed', handleDepFailed as EventListener);
		window.addEventListener('nebo:dep_pending', handleDepPending as EventListener);
		window.addEventListener('nebo:dep_cascade_complete', handleDepCascadeComplete as EventListener);
	});

	onDestroy(() => {
		window.removeEventListener('nebo:plugin_auth_complete', handleAuthComplete as EventListener);
		window.removeEventListener('nebo:plugin_auth_error', handleAuthError as EventListener);
		window.removeEventListener('nebo:plugin_auth_url', handleAuthUrl as EventListener);
		window.removeEventListener('nebo:dep_started', handleDepStarted as EventListener);
		window.removeEventListener('nebo:dep_installed', handleDepInstalled as EventListener);
		window.removeEventListener('nebo:dep_failed', handleDepFailed as EventListener);
		window.removeEventListener('nebo:dep_pending', handleDepPending as EventListener);
		window.removeEventListener('nebo:dep_cascade_complete', handleDepCascadeComplete as EventListener);
		if (depTimeout) clearTimeout(depTimeout);
	});

	function summarizeTrigger(wf: AgentWorkflow): string {
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
			if (existingAgentId) {
				agentId = existingAgentId;
			} else {
				// The install response carries the installed agent id (== product id),
				// so we address it directly instead of matching by display name.
				const res = await installStoreProduct(appId);
				agentId = res?.agentId || appId;
			}

			// Persist any collected inputs up front.
			if (Object.keys(inputValues).length > 0) {
				await updateAgentInputs(agentId, inputValues).catch(() => {});
			}

			// Show per-dependency install progress, driven by dep_* WS events.
			// Configure mode: deps are already installed — skip straight to setup.
			depRows = configuring ? [] : normalizeDeps();
			if (depRows.length > 0) {
				depsAdvanced = false;
				step = 'installing-deps';
				// Safety net: advance even if no terminal cascade event arrives.
				depTimeout = setTimeout(() => finishDeps(), 30000);
			} else {
				await afterDepsInstalled();
			}
		} catch (e: any) {
			error = e?.error || e?.message || 'Failed to install agent';
			step = 'inputs';
		}
	}

	/** Post-dependency-install: load setup data and route to auth / schedule / done. */
	async function afterDepsInstalled() {
		if (depTimeout) { clearTimeout(depTimeout); depTimeout = undefined; }
		try {
			let pluginsNeedingAuth: PluginAuthEntry[] = [];
			try {
				const agentRes = await getAgent(agentId);
				if (agentRes?.inputFields) {
					inputFields = agentRes.inputFields as AgentInputField[];
				}
				if (Array.isArray(agentRes?.pluginsNeedingAuth)) {
					pluginsNeedingAuth = agentRes.pluginsNeedingAuth as PluginAuthEntry[];
				}
			} catch { /* ignore */ }

			try {
				const wfRes = await listAgentWorkflows(agentId);
				const wfList = wfRes?.workflows;
				workflows = Array.isArray(wfList) ? wfList as AgentWorkflow[] : [];
			} catch { /* ignore */ }

			if (pluginsNeedingAuth.length > 0) {
				authQueue = pluginsNeedingAuth;
				authIndex = 0;
				step = 'auth';
			} else if (hasSchedules) {
				step = 'schedule';
			} else {
				await finalize();
			}
		} catch (e: any) {
			error = e?.error || e?.message || 'Failed to set up agent';
			step = 'inputs';
		}
	}

	async function startAuth() {
		if (!currentAuthPlugin) return;
		authInProgress = true;
		error = '';
		try {
			await authLogin(currentAuthPlugin.slug);
		} catch (e: any) {
			authInProgress = false;
			error = e?.error || e?.message || 'Failed to start authentication';
		}
	}

	function advanceAuth() {
		if (authIndex + 1 < authQueue.length) {
			authIndex++;
			error = '';
		} else {
			// All auth done — proceed to next step
			if (hasSchedules) {
				step = 'schedule';
			} else {
				finalize();
			}
		}
	}

	function skipAuth() {
		advanceAuth();
	}

	async function handleScheduleDone() {
		step = 'installing';
		try {
			for (const [bindingName, interval] of Object.entries(scheduleOverrides)) {
				const wf = workflows.find(w => w.bindingName === bindingName);
				if (!wf) continue;

				if (wf.triggerType === 'heartbeat') {
					const parts = wf.triggerConfig.split('|');
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

		{:else if step === 'installing-deps'}
			<div class="px-6 py-8 overflow-y-auto">
				<div class="text-center mb-6">
					<h2 class="font-display text-xl font-bold">Installing components</h2>
					<p class="text-xs text-base-content/50 mt-1">{installedDeps} of {depRows.length} ready</p>
				</div>
				<div class="flex flex-col gap-2.5">
					{#each depRows as dep (dep.reference)}
						<div class="flex items-center gap-3 rounded-xl border border-base-content/10 p-3">
							{#if dep.state === 'done'}
								<div class="w-9 h-9 shrink-0 rounded-full bg-success/15 flex items-center justify-center">
									<svg class="w-4 h-4 text-success" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12" /></svg>
								</div>
							{:else if dep.state === 'failed'}
								<div class="w-9 h-9 shrink-0 rounded-full bg-error/15 flex items-center justify-center">
									<X class="w-4 h-4 text-error" />
								</div>
							{:else if dep.state === 'installing'}
								<div class="w-9 h-9 shrink-0 rounded-full bg-primary/10 flex items-center justify-center">
									<span class="loading loading-spinner loading-sm text-primary"></span>
								</div>
							{:else}
								<div class="w-9 h-9 shrink-0 rounded-full bg-base-content/10 flex items-center justify-center">
									<span class="w-2 h-2 rounded-full bg-base-content/40"></span>
								</div>
							{/if}
							<div class="min-w-0 flex-1">
								<p class="text-sm font-medium truncate">{dep.label}</p>
								<p class="text-xs text-base-content/70 capitalize">
									{dep.depType}{dep.state === 'failed' ? ' · failed' : dep.state === 'done' ? ' · ready' : dep.state === 'installing' ? ' · installing…' : ' · waiting'}
								</p>
								{#if dep.error}<p class="text-xs text-error mt-0.5 truncate">{dep.error}</p>{/if}
							</div>
						</div>
					{/each}
				</div>
			</div>

		{:else if step === 'done'}
			<div class="flex flex-col items-center justify-center py-16 px-6">
				<svg class="w-12 h-12 text-success" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" /><polyline points="22 4 12 14.01 9 11.01" />
				</svg>
				<p class="text-base font-medium mt-4">{agentName} is ready!</p>
				<p class="text-sm text-base-content/70 mt-1">Your agent is now active and working.</p>
			</div>

		{:else if step === 'auth'}
			<div class="flex items-center justify-between px-6 pt-6 pb-2">
				<div></div>
				<button type="button" class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors" onclick={onCancel} aria-label="Close">
					<X class="w-4 h-4 text-base-content/70" />
				</button>
			</div>

			<div class="px-6 pb-6 overflow-y-auto">
				<div class="text-center mb-6">
					<div class="w-12 h-12 rounded-full bg-primary/15 flex items-center justify-center mx-auto mb-4">
						<KeyRound class="w-6 h-6 text-primary" />
					</div>
					<h2 class="font-display text-xl font-bold">Connect account</h2>
					{#if authQueue.length > 1}
						<p class="text-xs text-base-content/50 mt-1">Step {authIndex + 1} of {authQueue.length}</p>
					{/if}
				</div>

				{#if error}
					<div class="text-sm text-error bg-error/10 rounded-lg px-3 py-2 mb-4">{error}</div>
				{/if}

				{#if currentAuthPlugin}
					<div class="rounded-xl border border-base-content/10 p-4 mb-6">
						<p class="text-sm font-medium">{currentAuthPlugin.label || currentAuthPlugin.slug}</p>
						{#if currentAuthPlugin.description}
							<p class="text-xs text-base-content/70 mt-1">{currentAuthPlugin.description}</p>
						{/if}
					</div>
				{/if}

				{#if authInProgress}
					<div class="flex flex-col items-center py-4 mb-6">
						<span class="loading loading-spinner loading-md text-primary"></span>
						<p class="text-sm text-base-content/70 mt-3">Waiting for authorization...</p>
						<p class="text-xs text-base-content/50 mt-1">Complete the sign-in in your browser, then return here.</p>
					</div>

					<div class="flex justify-center">
						<button type="button" class="text-sm text-base-content/50 hover:text-base-content/70 transition-colors" onclick={() => { authInProgress = false; }}>
							Cancel
						</button>
					</div>
				{:else}
					<div class="flex gap-3">
						<button type="button" class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors" onclick={skipAuth}>
							Skip
						</button>
						<button type="button" class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all" onclick={startAuth}>
							Connect {currentAuthPlugin?.label || 'Account'}
						</button>
					</div>
				{/if}
			</div>

		{:else if step === 'schedule'}
			<div class="flex items-center justify-between px-6 pt-6 pb-2">
				<div></div>
				<button type="button" class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors" onclick={onCancel} aria-label="Close">
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
					<button type="button" class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors" onclick={() => { step = 'inputs'; }}>
						Back
					</button>
					<button type="button" class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all" onclick={handleScheduleDone}>
						Start working
					</button>
				</div>
			</div>

		{:else}
			<div class="flex items-center justify-between px-6 pt-6 pb-2">
				<div></div>
				<button type="button" class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors" onclick={onCancel} aria-label="Close">
					<X class="w-4 h-4 text-base-content/70" />
				</button>
			</div>

			<div class="px-6 pb-6 overflow-y-auto">
				<div class="text-center mb-6">
					<h2 class="font-display text-xl font-bold">{configuring ? 'Configure' : 'Set up'} {agentName}</h2>
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
					<button type="button" class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors" onclick={onCancel}>
						Cancel
					</button>
					<button type="button" class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all" onclick={handleInstall}>
						{configuring ? 'Save changes' : (hasInputFields || Object.keys(inputs).length > 0 ? 'Next' : 'Install & Start')}
					</button>
				</div>
				{#if configuring && onUninstall}
					<button type="button" onclick={onUninstall} class="w-full mt-3 h-9 text-sm font-medium text-error/80 hover:text-error transition-colors">
						Uninstall {agentName}
					</button>
				{/if}
			</div>
		{/if}
	</div>
</div>
