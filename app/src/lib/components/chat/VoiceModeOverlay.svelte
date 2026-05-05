<script lang="ts">
	import { onDestroy } from 'svelte';
	import { voiceSession } from '$lib/stores/voiceSession';
	import { logger } from '$lib/monitoring';
	import X from 'lucide-svelte/icons/x';
	import Mic from 'lucide-svelte/icons/mic';
	import MicOff from 'lucide-svelte/icons/mic-off';
	import Square from 'lucide-svelte/icons/square';

	const log = logger.child({ component: 'VoiceModeOverlay' });

	let { agentId, agentName, onclose }: {
		agentId: string;
		agentName: string;
		onclose: () => void;
	} = $props();

	let status = $derived($voiceSession.status);
	let audioLevel = $derived($voiceSession.audioLevel);
	let transcripts = $derived($voiceSession.transcripts);
	let interimTranscript = $derived($voiceSession.interimTranscript);
	let isMuted = $derived($voiceSession.isMuted);
	let errorMessage = $derived($voiceSession.errorMessage);

	let transcriptEl: HTMLDivElement | null = $state(null);

	// Auto-start voice session on mount
	$effect(() => {
		if (status === 'idle') {
			log.info('VoiceModeOverlay mounted, starting session for agent: ' + agentId);
			voiceSession.start(agentId);
		}
	});

	// Auto-scroll transcript area when new messages arrive
	$effect(() => {
		// Touch transcripts to subscribe to changes
		transcripts.length;
		interimTranscript;

		if (transcriptEl) {
			// Use requestAnimationFrame to wait for DOM update
			requestAnimationFrame(() => {
				if (transcriptEl) {
					transcriptEl.scrollTop = transcriptEl.scrollHeight;
				}
			});
		}
	});

	// Cleanup on destroy
	onDestroy(() => {
		voiceSession.stop();
	});

	function handleClose() {
		voiceSession.stop();
		onclose();
	}

	function handleToggleMute() {
		voiceSession.toggleMute();
	}

	function handleStop() {
		voiceSession.stop();
		onclose();
	}

	function handleInterrupt() {
		voiceSession.interrupt();
	}

	// Status display text
	let statusText = $derived(
		status === 'connecting' ? 'Connecting...'
		: status === 'listening' ? (isMuted ? 'Muted' : 'Listening...')
		: status === 'processing' ? 'Thinking...'
		: status === 'speaking' ? 'Speaking...'
		: status === 'error' ? 'Error'
		: ''
	);

	// Audio visualization scale (maps 0-1 audioLevel to a pulse size)
	let pulseScale = $derived(
		status === 'listening' && !isMuted
			? 1 + audioLevel * 0.6
			: status === 'speaking'
				? 1.3
				: 1
	);
</script>

<!-- Full-screen overlay -->
<div class="fixed inset-0 z-50 bg-base-100 flex flex-col">
	<!-- Header -->
	<div class="flex items-center justify-between px-6 py-4">
		<div class="w-10"></div>
		<div class="text-center">
			<div class="text-base font-semibold">{agentName}</div>
			<div class="text-xs text-base-content/50 mt-0.5 h-4">
				{#if status === 'error' && errorMessage}
					<span class="text-error">{errorMessage}</span>
				{:else}
					{statusText}
				{/if}
			</div>
		</div>
		<button
			class="w-10 h-10 rounded-lg grid place-items-center text-base-content/60 hover:text-base-content hover:bg-base-200/50 cursor-pointer transition-colors border-none bg-transparent"
			title="Close voice mode"
			onclick={handleClose}
		>
			<X class="w-5 h-5" />
		</button>
	</div>

	<!-- Center area: Audio visualization + status -->
	<div class="flex-1 flex flex-col items-center justify-center gap-8 min-h-0">
		<!-- Audio level visualization circle -->
		<button
			class="relative w-32 h-32 rounded-full grid place-items-center cursor-pointer border-none bg-transparent transition-transform duration-150 ease-out"
			onclick={status === 'speaking' ? handleInterrupt : undefined}
			title={status === 'speaking' ? 'Tap to interrupt' : ''}
		>
			<!-- Outer pulse ring -->
			<div
				class="absolute inset-0 rounded-full bg-primary/10 transition-transform duration-150 ease-out"
				class:scale-100={pulseScale <= 1}
				class:scale-110={pulseScale > 1 && pulseScale <= 1.2}
				class:scale-125={pulseScale > 1.2 && pulseScale <= 1.4}
				class:scale-150={pulseScale > 1.4}
			></div>

			<!-- Inner circle -->
			<div class="relative w-24 h-24 rounded-full grid place-items-center {
				status === 'connecting' ? 'bg-base-300' :
				status === 'listening' ? (isMuted ? 'bg-base-300' : 'bg-primary/20') :
				status === 'processing' ? 'bg-warning/20' :
				status === 'speaking' ? 'bg-primary/30' :
				status === 'error' ? 'bg-error/20' :
				'bg-base-300'
			}">
				{#if status === 'connecting'}
					<div class="loading loading-spinner loading-lg text-base-content/40"></div>
				{:else if status === 'listening'}
					{#if isMuted}
						<MicOff class="w-10 h-10 text-base-content/40" />
					{:else}
						<Mic class="w-10 h-10 text-primary" />
					{/if}
				{:else if status === 'processing'}
					<div class="loading loading-dots loading-lg text-warning"></div>
				{:else if status === 'speaking'}
					<div class="flex items-end gap-1 h-10">
						<div class="w-1.5 bg-primary rounded-full voice-pulse h-4"></div>
						<div class="w-1.5 bg-primary rounded-full voice-pulse h-7"></div>
						<div class="w-1.5 bg-primary rounded-full voice-pulse h-5"></div>
						<div class="w-1.5 bg-primary rounded-full voice-pulse h-8"></div>
						<div class="w-1.5 bg-primary rounded-full voice-pulse h-3"></div>
					</div>
				{:else if status === 'error'}
					<span class="text-2xl text-error">!</span>
				{/if}
			</div>
		</button>

		{#if status === 'speaking'}
			<button
				class="text-xs text-base-content/50 hover:text-base-content cursor-pointer border-none bg-transparent transition-colors"
				onclick={handleInterrupt}
			>
				Tap to interrupt
			</button>
		{/if}
	</div>

	<!-- Transcript area -->
	<div class="flex-shrink-0 max-h-64 px-6 pb-2">
		<div
			class="overflow-y-auto max-h-56 space-y-3 px-2"
			bind:this={transcriptEl}
		>
			{#each transcripts as entry, i (i)}
				<div class="flex flex-col {entry.speaker === 'user' ? 'items-end' : 'items-start'}">
					<div class="text-xs text-base-content/50 mb-0.5 font-mono">
						{entry.speaker === 'user' ? 'You' : agentName}
					</div>
					<div class="text-sm max-w-md {entry.speaker === 'user' ? 'text-base-content' : 'text-base-content/90'}">
						{entry.text}
					</div>
				</div>
			{/each}

			<!-- Interim (live) transcript -->
			{#if interimTranscript}
				<div class="flex flex-col items-end">
					<div class="text-xs text-base-content/50 mb-0.5 font-mono">You</div>
					<div class="text-sm text-base-content/50 italic max-w-md">
						{interimTranscript}
					</div>
				</div>
			{/if}
		</div>
	</div>

	<!-- Bottom controls -->
	<div class="flex items-center justify-center gap-6 px-6 py-6 border-t border-base-content/10">
		<!-- Mute toggle -->
		<button
			class="w-14 h-14 rounded-full grid place-items-center cursor-pointer transition-colors border-none {
				isMuted
					? 'bg-error/20 text-error'
					: 'bg-base-200 text-base-content/70 hover:bg-base-300 hover:text-base-content'
			}"
			title={isMuted ? 'Unmute' : 'Mute'}
			onclick={handleToggleMute}
			disabled={status === 'connecting' || status === 'idle' || status === 'error'}
		>
			{#if isMuted}
				<MicOff class="w-6 h-6" />
			{:else}
				<Mic class="w-6 h-6" />
			{/if}
		</button>

		<!-- Stop button -->
		<button
			class="w-16 h-16 rounded-full grid place-items-center bg-error text-error-content cursor-pointer transition-colors hover:bg-error/80 border-none"
			title="End conversation"
			onclick={handleStop}
		>
			<Square class="w-6 h-6 fill-current" />
		</button>
	</div>
</div>
