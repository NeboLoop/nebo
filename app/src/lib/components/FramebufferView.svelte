<script lang="ts">
	import { framebufferStore } from '$lib/stores/framebuffer';
	import { onMount, onDestroy } from 'svelte';
	import Monitor from 'lucide-svelte/icons/monitor';

	let {
		sessionId,
		width = 1024,
		height = 768
	}: {
		sessionId: string;
		width?: number;
		height?: number;
	} = $props();

	let canvasEl: HTMLCanvasElement;
	let canvasId = crypto.randomUUID();

	onMount(() => {
		if (!canvasEl) return;

		// Transfer canvas to worker as OffscreenCanvas
		const offscreen = canvasEl.transferControlToOffscreen();
		framebufferStore.adoptCanvas(canvasId, offscreen);

		// Bind session to canvas for frame rendering
		framebufferStore.bindSession(sessionId, canvasId);
	});

	onDestroy(() => {
		framebufferStore.unbindSession(sessionId);
		framebufferStore.releaseCanvas(canvasId);
	});
</script>

<div class="relative rounded-lg overflow-hidden border border-base-300 bg-black">
	<canvas bind:this={canvasEl} {width} {height} class="w-full h-auto"></canvas>
	{#if !$framebufferStore.isConnected}
		<div class="absolute inset-0 flex items-center justify-center bg-base-300/50">
			<Monitor class="w-8 h-8 text-base-content/30" />
		</div>
	{/if}
</div>
