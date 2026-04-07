<!--
  CodeInstallModal — shows install progress when redeeming SKIL/PLUG/ROLE/WORK/LOOP codes.
  Replaces the plain-text chat messages with a live-updating modal.
-->

<script lang="ts">
	import { onDestroy } from 'svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { Check, CircleAlert, CreditCard, KeyRound, Loader2 } from 'lucide-svelte';
	import { pluginAuthLogin } from '$lib/api/nebo';

	type Phase = 'installing' | 'auth' | 'done' | 'error' | 'payment';

	type DepItem = {
		reference: string;
		type: string;
		status: 'pending' | 'installing' | 'installed' | 'failed';
		error?: string;
	};

	let {
		show = $bindable(false),
		onclose,
		onAgentSetup
	}: {
		show: boolean;
		onclose?: () => void;
		onAgentSetup?: (agentId: string, agentName: string) => void;
	} = $props();

	let phase = $state<Phase>('installing');
	let code = $state('');
	let codeType = $state('');
	let artifactName = $state('');
	let statusMessage = $state('');
	let errorMessage = $state('');
	let checkoutUrl = $state('');
	let deps = $state<DepItem[]>([]);
	let depsSummary = $state({ installed: 0, pending: 0, failed: 0 });
	let authLabel = $state('');
	let authDescription = $state('');
	let authInProgress = $state(false);
	let pluginSlug = $state('');
	let authQueue = $state<Array<{slug: string, label: string, description: string}>>([]);
	let authIndex = $state(0);
	let pendingAgentId = $state('');

	const typeLabel = $derived(codeType ? codeType.charAt(0).toUpperCase() + codeType.slice(1) : 'Code');
	const title = $derived(
		phase === 'done'
			? `${typeLabel} Installed`
			: phase === 'error'
				? 'Install Failed'
				: phase === 'payment'
					? 'Payment Required'
					: phase === 'auth'
						? `Connect ${authLabel || 'Account'}`
						: `Installing ${typeLabel}`
	);
	const installedCount = $derived(deps.filter((d) => d.status === 'installed').length);
	const canClose = $derived(phase !== 'installing' || installElapsed >= 10);

	// Allow closing the modal if install has been running for more than 10 seconds
	let installElapsed = $state(0);
	let installTimer: ReturnType<typeof setInterval> | null = null;

	function startInstallTimer() {
		installElapsed = 0;
		stopInstallTimer();
		installTimer = setInterval(() => { installElapsed++; }, 1000);
	}

	function stopInstallTimer() {
		if (installTimer) { clearInterval(installTimer); installTimer = null; }
	}

	onDestroy(stopInstallTimer);

	function reset() {
		phase = 'installing';
		code = '';
		codeType = '';
		artifactName = '';
		statusMessage = '';
		errorMessage = '';
		checkoutUrl = '';
		deps = [];
		depsSummary = { installed: 0, pending: 0, failed: 0 };
		authLabel = '';
		authDescription = '';
		authInProgress = false;
		pluginSlug = '';
		authQueue = [];
		authIndex = 0;
		pendingAgentId = '';
		stopInstallTimer();
	}

	function findOrAddDep(reference: string, type: string): number {
		const idx = deps.findIndex((d) => d.reference === reference);
		if (idx >= 0) return idx;
		deps = [...deps, { reference, type, status: 'pending' }];
		return deps.length - 1;
	}

	// --- Public handlers called from Chat.svelte ---

	export function onCodeProcessing(data: Record<string, unknown>) {
		reset();
		code = (data?.code as string) || '';
		codeType = (data?.code_type as string) || '';
		statusMessage = (data?.status_message as string) || 'Processing...';
		show = true;
		startInstallTimer();
	}

	export function onPluginInstalling(data: Record<string, unknown>) {
		const plugin = (data?.plugin as string) || '';
		if (!plugin) return;
		const idx = findOrAddDep(plugin, 'plugin');
		deps[idx] = { ...deps[idx], status: 'installing' };
		deps = deps;
	}

	export function onPluginInstalled(data: Record<string, unknown>) {
		const plugin = (data?.plugin as string) || '';
		if (!plugin) return;
		const idx = findOrAddDep(plugin, 'plugin');
		deps[idx] = { ...deps[idx], status: 'installed' };
		deps = deps;
	}

	export function onPluginError(data: Record<string, unknown>) {
		const plugin = (data?.plugin as string) || '';
		const error = (data?.error as string) || 'Unknown error';
		if (!plugin) return;
		const idx = findOrAddDep(plugin, 'plugin');
		deps[idx] = { ...deps[idx], status: 'failed', error };
		deps = deps;
	}

	export function onDepPending(data: Record<string, unknown>) {
		const reference = (data?.reference as string) || '';
		const depType = ((data?.depType as string) || 'skill').toLowerCase();
		if (!reference) return;
		findOrAddDep(reference, depType);
	}

	export function onDepInstalled(data: Record<string, unknown>) {
		const reference = (data?.reference as string) || '';
		const depType = ((data?.depType as string) || 'skill').toLowerCase();
		if (!reference) return;
		const idx = findOrAddDep(reference, depType);
		deps[idx] = { ...deps[idx], status: 'installed' };
		deps = deps;
	}

	export function onDepFailed(data: Record<string, unknown>) {
		const reference = (data?.reference as string) || '';
		const depType = ((data?.depType as string) || 'skill').toLowerCase();
		const error = (data?.error as string) || 'Unknown error';
		if (!reference) return;
		const idx = findOrAddDep(reference, depType);
		deps[idx] = { ...deps[idx], status: 'failed', error };
		deps = deps;
	}

	export function onDepCascadeComplete(data: Record<string, unknown>) {
		depsSummary = {
			installed: (data?.installed as number) || 0,
			pending: (data?.pending as number) || 0,
			failed: (data?.failed as number) || 0
		};
	}

	export function onAgentAuthRequired(data: Record<string, unknown>) {
		const plugins = (data?.plugins as Array<{slug: string, label: string, description: string}>) || [];
		if (plugins.length === 0) return;
		authQueue = plugins;
		authIndex = 0;
		pluginSlug = plugins[0].slug;
		authLabel = plugins[0].label;
		authDescription = plugins[0].description || '';
		phase = 'auth';
	}

	export function onPluginAuthRequired(data: Record<string, unknown>) {
		pluginSlug = (data?.plugin as string) || '';
		authLabel = (data?.label as string) || 'Account';
		authDescription = (data?.description as string) || '';
		phase = 'auth';
	}

	export function onPluginAuthUrl(data: Record<string, unknown>) {
		const url = data?.url as string;
		if (url) {
			window.open(url, '_blank');
		}
	}

	export function onPluginAuthComplete(_data: Record<string, unknown>) {
		authInProgress = false;

		// Multi-plugin auth queue: advance to next plugin
		if (authQueue.length > 0) {
			authIndex++;
			if (authIndex < authQueue.length) {
				const next = authQueue[authIndex];
				pluginSlug = next.slug;
				authLabel = next.label;
				authDescription = next.description || '';
				return; // stay in auth phase for next plugin
			}
			// All auth done — hand off to agent setup if this was an agent install
			if (pendingAgentId && onAgentSetup) {
				show = false;
				onAgentSetup(pendingAgentId, artifactName);
				return;
			}
		}

		// Default: transition to done (existing behavior for direct plugin installs)
		phase = 'done';
		setTimeout(() => {
			show = false;
			onclose?.();
		}, 1500);
	}

	export function onPluginAuthError(data: Record<string, unknown>) {
		authInProgress = false;
		errorMessage = (data?.error as string) || 'Authentication failed';
		phase = 'error';
	}

	async function startAuth() {
		if (!pluginSlug) return;
		authInProgress = true;
		try {
			await pluginAuthLogin(pluginSlug);
		} catch {
			authInProgress = false;
			errorMessage = 'Failed to start authentication';
			phase = 'error';
		}
	}

	function skipAuth() {
		// Multi-plugin queue: advance to next or finish
		if (authQueue.length > 0) {
			authIndex++;
			if (authIndex < authQueue.length) {
				const next = authQueue[authIndex];
				pluginSlug = next.slug;
				authLabel = next.label;
				authDescription = next.description || '';
				return;
			}
			if (pendingAgentId && onAgentSetup) {
				show = false;
				onAgentSetup(pendingAgentId, artifactName);
				return;
			}
		}
		phase = 'done';
		setTimeout(() => {
			show = false;
			onclose?.();
		}, 1500);
	}

	export function onCodeResult(data: Record<string, unknown>) {
		stopInstallTimer();
		const success = data?.success as boolean;
		const paymentRequired = data?.payment_required as boolean;
		const checkout = data?.checkout_url as string | undefined;
		const name = (data?.artifact_name as string) || '';
		const agentId = (data?.artifact_id as string) || '';
		const error = (data?.error as string) || '';
		const message = (data?.message as string) || '';
		const needsAuth = data?.needsAuth as boolean;

		if (name) artifactName = name;

		// Store agent ID for deferred hand-off after auth completes
		if (codeType === 'agent' && agentId) {
			pendingAgentId = agentId;
		}

		if (success && paymentRequired && checkout) {
			checkoutUrl = checkout;
			phase = 'payment';
		} else if (success) {
			// If auth wizard is active, don't hand off yet — auth handlers will finish
			if (phase === 'auth') {
				return;
			}
			// Agent code install: hand off to agent setup modal for input configuration
			if (codeType === 'agent' && agentId && onAgentSetup) {
				show = false;
				onAgentSetup(agentId, artifactName);
				return;
			}
			statusMessage = message || `${artifactName || typeLabel} installed`;
			phase = 'done';
			setTimeout(() => {
				show = false;
				onclose?.();
			}, 1500);
		} else {
			errorMessage = error || 'Installation failed';
			phase = 'error';
		}
	}
</script>

<Modal
	bind:show
	{title}
	size="sm"
	closeOnBackdrop={canClose}
	closeOnEscape={canClose}
	showCloseButton={canClose}
	onclose={() => { show = false; onclose?.(); }}
>
	{#if phase === 'installing'}
		<div class="flex flex-col items-center gap-4 py-6">
			<span class="loading loading-spinner loading-lg text-primary"></span>
			<div class="text-center">
				<p class="text-base font-medium">{statusMessage}</p>
				{#if code}
					<p class="text-sm text-base-content/50 mt-1 font-mono">{code}</p>
				{/if}
			</div>
			{#if installElapsed >= 10}
				<button type="button" class="btn btn-sm btn-ghost" onclick={() => { show = false; stopInstallTimer(); onclose?.(); }}>
					Cancel
				</button>
			{/if}
		</div>

	{:else if phase === 'auth'}
		<div class="flex flex-col items-center gap-4 py-6">
			{#if authQueue.length > 1}
				<p class="text-xs text-base-content/50">Step {authIndex + 1} of {authQueue.length}</p>
			{/if}
			{#if authInProgress}
				<span class="loading loading-spinner loading-lg text-primary"></span>
				<div class="text-center">
					<p class="text-base font-medium">Waiting for authorization...</p>
					<p class="text-sm text-base-content/50 mt-1">Complete the sign-in in your browser, then return here.</p>
				</div>
				<button type="button" class="btn btn-sm btn-ghost mt-2" onclick={() => { authInProgress = false; phase = 'done'; setTimeout(() => { show = false; onclose?.(); }, 1500); }}>
					Cancel
				</button>
			{:else}
				<div class="w-12 h-12 rounded-full bg-primary/15 flex items-center justify-center">
					<KeyRound class="w-6 h-6 text-primary" />
				</div>
				<div class="text-center">
					{#if authDescription}
						<p class="text-sm text-base-content/70 mt-1">{authDescription}</p>
					{/if}
				</div>
				<button type="button" class="btn btn-primary btn-sm mt-2" onclick={startAuth}>
					Connect {authLabel || 'Account'}
				</button>
				<button type="button" class="btn btn-sm btn-ghost" onclick={skipAuth}>
					Skip
				</button>
			{/if}
		</div>

	{:else if phase === 'done'}
		<div class="flex flex-col items-center gap-4 py-6">
			<div class="w-12 h-12 rounded-full bg-success/15 flex items-center justify-center">
				<Check class="w-6 h-6 text-success" />
			</div>
			<p class="text-base font-medium">{artifactName || typeLabel} installed!</p>
		</div>

	{:else if phase === 'error'}
		<div class="flex flex-col items-center gap-4 py-6">
			<div class="w-12 h-12 rounded-full bg-error/15 flex items-center justify-center">
				<CircleAlert class="w-6 h-6 text-error" />
			</div>
			<div class="text-center">
				<p class="text-base font-medium">Failed to install</p>
				<p class="text-sm text-error/80 mt-2 max-w-sm">{errorMessage}</p>
			</div>
		</div>

	{:else if phase === 'payment'}
		<div class="flex flex-col items-center gap-4 py-6">
			<div class="w-12 h-12 rounded-full bg-warning/15 flex items-center justify-center">
				<CreditCard class="w-6 h-6 text-warning" />
			</div>
			<div class="text-center">
				<p class="text-base font-medium">Payment required</p>
				<p class="text-sm text-base-content/70 mt-1">
					<span class="font-medium">{artifactName || typeLabel}</span> is a paid artifact.
				</p>
			</div>
			{#if checkoutUrl}
				<a href={checkoutUrl} target="_blank" rel="noopener noreferrer" class="btn btn-primary btn-sm mt-2">
					Complete Checkout
				</a>
			{/if}
		</div>
	{/if}

	<!-- Dependency list -->
	{#if deps.length > 0}
		<div class="border-t border-base-content/10 pt-4 mt-2">
			<p class="text-xs font-medium text-base-content/50 uppercase tracking-wide mb-3">
				Dependencies
				{#if phase === 'done'}
					({installedCount}/{deps.length})
				{/if}
			</p>
			<ul class="space-y-2">
				{#each deps as dep}
					<li class="flex items-center gap-2.5 text-sm">
						{#if dep.status === 'installed'}
							<Check class="w-4 h-4 text-success shrink-0" />
						{:else if dep.status === 'installing'}
							<Loader2 class="w-4 h-4 text-primary animate-spin shrink-0" />
						{:else if dep.status === 'failed'}
							<CircleAlert class="w-4 h-4 text-error shrink-0" />
						{:else}
							<div class="w-4 h-4 rounded-full border-2 border-base-content/20 shrink-0"></div>
						{/if}
						<span class="truncate {dep.status === 'failed' ? 'text-error/80' : ''}">{dep.reference}</span>
						<span class="text-xs text-base-content/40 shrink-0">{dep.type}</span>
					</li>
				{/each}
			</ul>
		</div>
	{/if}

	{#snippet footer()}
		{#if phase === 'error' || phase === 'payment'}
			<button type="button" class="btn btn-sm btn-ghost" onclick={() => { show = false; onclose?.(); }}>
				Close
			</button>
		{/if}
	{/snippet}
</Modal>
