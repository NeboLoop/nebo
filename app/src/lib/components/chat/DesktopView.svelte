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
	import { ownScreen } from '$lib/stores/ownScreen';
	import { BOOT_DEADLINE_MS, afterDisconnect, onFirstFrame, type DesktopStatus } from '$lib/chat/desktopBoot';

	let {
		onclose,
		onrecord,
		recording = false
	}: { onclose?: () => void; onrecord?: () => void; recording?: boolean } = $props();

	let container: HTMLDivElement | undefined = $state();
	// Opening the computer is plain view mode: recording is only ever the
	// owner's own choice (the Record task button). Until the desktop is all
	// the way up and its first frame is drawn, the view stays covered by ONE
	// loading state ($lib/chat/desktopBoot).
	let status = $state<DesktopStatus>('starting');
	let rfb: RFB | null = null;
	let closed = false;

	// Redials are bounded: /ws/desktop 404s off Linux, so a host bot with no
	// virtual computer must not redial forever (see afterDisconnect).
	let attempts = 0;
	let everReady = false;
	let bootTimer: ReturnType<typeof setTimeout> | null = null;
	/** Bumped by every (re)boot, so a redial queued by an earlier one is dropped. */
	let generation = 0;

	function clearBoot() {
		if (bootTimer) {
			clearTimeout(bootTimer);
			bootTimer = null;
		}
	}

	function boot() {
		clearBoot();
		generation += 1;
		status = 'starting';
		attempts = 0;
		everReady = false;
		bootTimer = setTimeout(() => {
			bootTimer = null;
			if (closed || status === 'ready') return;
			generation += 1;
			status = 'failed';
			const r = rfb;
			rfb = null;
			r?.disconnect();
		}, BOOT_DEADLINE_MS);
		connect();
	}

	function connect() {
		if (closed || !container) return;
		attempts += 1;
		container.replaceChildren();
		const r = new RFB(container, `${backendWsBase()}/ws/desktop`, { shared: true });
		r.scaleViewport = true;
		r.resizeSession = false;
		onFirstFrame(r, () => {
			if (rfb !== r || closed) return;
			clearBoot();
			status = 'ready';
			everReady = true;
			attempts = 0;
		});
		r.addEventListener('disconnect', () => {
			// A connection we already let go (closed, timed out, replaced)
			// says nothing about the one in view.
			if (rfb !== r || closed) return;
			rfb = null;
			const next = afterDisconnect(everReady, attempts, $ownScreen);
			status = next.status;
			if (!next.redial) {
				clearBoot();
				return;
			}
			// The desktop survives transport loss server-side (x11vnc -forever,
			// tunnel redial) — keep trying quietly instead of dying on a blip.
			const gen = generation;
			setTimeout(() => {
				if (!closed && gen === generation && rfb === null) connect();
			}, 3000);
		});
		rfb = r;
	}

	function retry() {
		const r = rfb;
		rfb = null;
		r?.disconnect();
		boot();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && onclose) {
			e.preventDefault();
			onclose();
		}
	}

	onMount(() => {
		boot();
		// noVNC only rescales on window resize — a panel drag or the opening
		// animation leaves the canvas frozen at its connect-moment size.
		// Re-assigning scaleViewport forces a rescale to the current box.
		const ro = new ResizeObserver(() => {
			if (rfb) rfb.scaleViewport = true;
		});
		if (container) ro.observe(container);
		return () => {
			closed = true;
			clearBoot();
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
			class="w-1.5 h-1.5 rounded-full {status === 'ready'
				? 'bg-success'
				: status === 'starting' || status === 'reconnecting'
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
		{:else if status === 'failed'}
			<div class="absolute inset-0 bg-neutral flex flex-col items-center justify-center gap-3 text-neutral-content/80 px-6 text-center" data-desktop-failed>
				<Monitor class="w-8 h-8" />
				<p class="text-sm">{$t('chat.computerStartFailed')}</p>
				<button type="button" class="btn btn-xs btn-neutral" onclick={retry}>{$t('chat.tryAgain')}</button>
			</div>
		{:else if status === 'starting'}
			<!-- Opaque until the first frame: nothing of a desktop that is still
			     coming up shows through. -->
			<div class="absolute inset-0 bg-neutral flex flex-col items-center justify-center gap-3 text-neutral-content/80 pointer-events-none" data-desktop-starting>
				<span class="loading loading-spinner loading-md"></span>
				<span class="text-sm">{$t('chat.startingComputer')}</span>
			</div>
		{:else if status === 'reconnecting'}
			<div class="absolute inset-0 flex flex-col items-center justify-center gap-2 text-neutral-content/80 pointer-events-none">
				<Monitor class="w-8 h-8" />
				<span class="text-sm">{$t('chat.computerReconnecting')}</span>
			</div>
		{/if}
	</div>
</div>
