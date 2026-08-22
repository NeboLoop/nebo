<script lang="ts">
	/**
	 * The bot's computer, live in the work panel. noVNC speaks RFB over
	 * /ws/desktop (a byte pipe to the pod's loopback x11vnc); the desktop
	 * starts on demand server-side when the first viewer connects. Watch or
	 * take the keyboard — input fidelity (modifiers, drag, clipboard) is
	 * RFB's, not ours.
	 */
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Monitor from 'lucide-svelte/icons/monitor';
	import RFB from '@novnc/novnc';
	import { backendWsBase } from '$lib/api/base';

	let {
		onclose,
		onrecord,
		recording = false
	}: { onclose?: () => void; onrecord?: () => void; recording?: boolean } = $props();

	let container: HTMLDivElement | undefined = $state();
	let status = $state<'connecting' | 'connected' | 'error' | 'unavailable'>('connecting');
	let rfb: RFB | null = null;
	let closed = false;

	// Retrying forever is right for a blip and wrong for a machine that has no
	// desktop at all: /ws/desktop 404s off Linux, so on a Mac or Windows install
	// the old loop reconnected every 3s for as long as the pane stayed open.
	// Never connected → give up after a few tries and say so. Connected once →
	// it's a real desktop, so keep redialling, but still bounded.
	const RETRIES_BEFORE_CONNECT = 3;
	const RETRIES_AFTER_CONNECT = 10;
	let attempts = 0;
	let everConnected = false;

	function connect() {
		if (closed || !container) return;
		status = 'connecting';
		attempts += 1;
		container.replaceChildren();
		const r = new RFB(container, `${backendWsBase()}/ws/desktop`, { shared: true });
		r.scaleViewport = true;
		r.resizeSession = false;
		r.addEventListener('connect', () => {
			status = 'connected';
			everConnected = true;
			attempts = 0;
		});
		r.addEventListener('disconnect', () => {
			rfb = null;
			if (closed) return;
			// The desktop survives transport loss server-side (x11vnc -forever,
			// tunnel redial) — keep trying quietly instead of dying on a blip.
			const limit = everConnected ? RETRIES_AFTER_CONNECT : RETRIES_BEFORE_CONNECT;
			if (attempts >= limit) {
				status = 'unavailable';
				return;
			}
			status = 'error';
			setTimeout(connect, 3000);
		});
		rfb = r;
	}

	function retry() {
		attempts = 0;
		connect();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && onclose) {
			e.preventDefault();
			onclose();
		}
	}

	onMount(() => {
		connect();
		// noVNC only rescales on window resize — a panel drag or the opening
		// animation leaves the canvas frozen at its connect-moment size.
		// Re-assigning scaleViewport forces a rescale to the current box.
		const ro = new ResizeObserver(() => {
			if (rfb) rfb.scaleViewport = true;
		});
		if (container) ro.observe(container);
		return () => {
			closed = true;
			ro.disconnect();
			rfb?.disconnect();
			rfb = null;
		};
	});

</script>

<svelte:window {onkeydown} />

<div class="flex flex-col h-full min-h-0">
	<!-- This bar sits on the full-window dark ground, so it carries its own
	     palette: base-content here would be dark text on a dark background, which
	     is exactly how the close button went missing. -->
	<div class="flex items-center gap-2 px-3 h-11 bg-neutral text-neutral-content border-b border-white/10 shrink-0">
		<Monitor class="w-4 h-4 opacity-70" />
		<span class="text-sm font-medium">{$t('chat.botComputer')}</span>
		<span
			class="w-1.5 h-1.5 rounded-full {status === 'connected'
				? 'bg-success'
				: status === 'connecting'
					? 'bg-warning animate-pulse'
					: 'bg-error'}"
		></span>
		<div class="flex-1"></div>
		{#if onrecord}
			<button
				type="button"
				class="btn btn-sm normal-case gap-1.5 {recording ? 'btn-error' : 'btn-ghost text-neutral-content hover:bg-white/10'}"
				onclick={onrecord}
			>
				<span class="w-2 h-2 rounded-full {recording ? 'bg-white animate-pulse' : 'bg-error'}"></span>
				{recording ? $t('chat.recording') : $t('chat.recordTask')}
			</button>
		{/if}
		{#if onclose}
			<button
				type="button"
				class="btn btn-sm normal-case gap-1.5 bg-white/15 hover:bg-white/25 border-none text-neutral-content"
				onclick={onclose}
				title="{$t('chat.closeComputer')} (Esc)"
			>
				<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
				{$t('common.close')}
			</button>
		{/if}
	</div>
	<div class="flex-1 min-h-0 bg-neutral relative">
		<div bind:this={container} class="absolute inset-0 [&_canvas]:outline-none"></div>
		{#if status === 'unavailable'}
			<div class="absolute inset-0 flex flex-col items-center justify-center gap-3 text-neutral-content/80 px-6 text-center">
				<Monitor class="w-8 h-8" />
				<div>
					<p class="text-sm">{$t('chat.computerUnavailable')}</p>
					<p class="text-xs text-neutral-content/60 mt-1">{$t('chat.computerUnavailableHint')}</p>
				</div>
				<button type="button" class="btn btn-xs btn-neutral" onclick={retry}>{$t('common.retry')}</button>
			</div>
		{:else if status !== 'connected'}
			<div class="absolute inset-0 flex flex-col items-center justify-center gap-2 text-neutral-content/80 pointer-events-none">
				<Monitor class="w-8 h-8" />
				<span class="text-sm">
					{status === 'connecting' ? $t('chat.startingComputer') : $t('chat.computerReconnecting')}
				</span>
			</div>
		{/if}
	</div>
</div>
