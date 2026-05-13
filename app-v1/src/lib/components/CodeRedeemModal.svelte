<!--
  CodeRedeemModal — form to enter and redeem marketplace install codes.
  idle (form) → installing → done/error.
-->

<script lang="ts">
	import Modal from '$lib/components/ui/Modal.svelte';
	import { Check, CircleAlert, Zap } from 'lucide-svelte';
	import { getWebSocketClient } from '$lib/websocket/client';

	let {
		show = $bindable(false),
		onclose
	}: {
		show: boolean;
		onclose?: () => void;
	} = $props();

	let value = $state('');
	let phase = $state<'idle' | 'installing' | 'done' | 'error'>('idle');
	let artifactName = $state('');
	let codeType = $state('');
	let errorMessage = $state('');
	let unsubs: Array<() => void> = [];

	const CODE_RX = /^(NEBO|SKIL|AGNT|LOOP|PLUG)-[0-9A-Z]{4}-[0-9A-Z]{4}$/i;
	const isValidCode = $derived(CODE_RX.test(value.trim()));
	const typeLabel = $derived(codeType ? codeType.charAt(0).toUpperCase() + codeType.slice(1) : 'Code');

	function reset() {
		value = '';
		phase = 'idle';
		artifactName = '';
		codeType = '';
		errorMessage = '';
		cleanupListeners();
	}

	function cleanupListeners() {
		unsubs.forEach((u) => u());
		unsubs = [];
	}

	function close() {
		cleanupListeners();
		show = false;
		onclose?.();
	}

	$effect(() => {
		if (show) {
			reset();
		}
		return () => cleanupListeners();
	});

	function submit() {
		const code = value.trim().toUpperCase();
		if (!CODE_RX.test(code)) return;

		const prefix = code.split('-')[0];
		codeType = { NEBO: 'nebo', SKIL: 'skill', AGNT: 'agent', LOOP: 'loop', PLUG: 'plugin' }[prefix] || 'code';
		phase = 'installing';

		const ws = getWebSocketClient();

		unsubs.push(
			ws.on('code_processing', (data: Record<string, unknown>) => {
				if (data?.artifact_name) artifactName = data.artifact_name as string;
			}),
			ws.on('code_result', (data: Record<string, unknown>) => {
				cleanupListeners();
				const success = data?.success as boolean;
				const name = (data?.artifact_name as string) || '';
				const error = (data?.error as string) || '';

				if (name) artifactName = name;

				if (success) {
					phase = 'done';
				} else {
					errorMessage = error || 'Installation failed';
					phase = 'error';
				}
			})
		);

		ws.send('chat', {
			session_id: '',
			prompt: code,
			companion: true
		});
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && isValidCode && phase === 'idle') {
			e.preventDefault();
			submit();
		}
	}
</script>

<Modal
	bind:show
	title="Redeem install code"
	size="sm"
	onclose={close}
>
	{#if phase === 'idle'}
		<p class="text-sm text-base-content/60 mb-4">Enter an install code from the marketplace to add agents, skills, or plugins.</p>
		<div class="relative">
			<Zap class="absolute left-3.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-base-content/30 pointer-events-none" />
			<input
				class="input input-bordered w-full pl-9 text-center font-mono text-base tracking-widest uppercase"
				bind:value
				placeholder="AGNT-XXXX-XXXX"
				spellcheck="false"
				autocomplete="off"
				onkeydown={handleKeydown}
			/>
		</div>
		<p class="text-xs text-base-content/40 text-center mt-2.5">
			Starts with <strong>AGNT</strong>, <strong>SKIL</strong>, <strong>PLUG</strong>, <strong>LOOP</strong>, or <strong>NEBO</strong>
		</p>

		{#if isValidCode}
			<div class="flex gap-2 mt-5">
				<button type="button" class="btn btn-ghost flex-1" onclick={close}>Cancel</button>
				<button type="button" class="btn btn-primary flex-1" onclick={submit}>Install</button>
			</div>
		{/if}

	{:else if phase === 'installing'}
		<div class="flex flex-col items-center gap-3 py-6">
			<span class="loading loading-spinner loading-lg text-primary"></span>
			<p class="text-base font-medium">Installing...</p>
			<p class="text-sm text-base-content/50 font-mono tracking-wide">{value.toUpperCase()}</p>
			<button type="button" class="btn btn-sm btn-ghost mt-2" onclick={close}>Cancel</button>
		</div>

	{:else if phase === 'done'}
		<div class="flex flex-col items-center gap-3 py-6">
			<div class="w-10 h-10 rounded-full bg-success text-white grid place-items-center">
				<Check class="w-5 h-5" />
			</div>
			<p class="text-base font-semibold text-success">{artifactName || typeLabel} installed</p>
			<p class="text-sm text-base-content/60">
				{#if codeType === 'agent'}
					Find it in your agents list.
				{:else if codeType === 'skill'}
					Attach it from the Skills rail in any agent.
				{:else}
					Ready to use.
				{/if}
			</p>
			<button type="button" class="btn btn-primary btn-sm mt-2" onclick={close}>Done</button>
		</div>

	{:else if phase === 'error'}
		<div class="flex flex-col items-center gap-3 py-6">
			<div class="w-10 h-10 rounded-full bg-error/15 text-error grid place-items-center">
				<CircleAlert class="w-5 h-5" />
			</div>
			<p class="text-base font-medium">Failed to install</p>
			<p class="text-sm text-error/80 text-center max-w-xs">{errorMessage}</p>
			<button type="button" class="btn btn-sm btn-ghost mt-2" onclick={() => { phase = 'idle'; }}>Try again</button>
		</div>
	{/if}
</Modal>
