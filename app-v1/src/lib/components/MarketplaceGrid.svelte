<script lang="ts">
	import { ChevronLeft, ChevronRight } from 'lucide-svelte';

	let { children } = $props<{ children: any }>();

	let gridEl: HTMLDivElement | undefined = $state();
	let canScrollLeft = $state(false);
	let canScrollRight = $state(false);

	function updateScrollState() {
		if (!gridEl) return;
		canScrollLeft = gridEl.scrollLeft > 1;
		canScrollRight = gridEl.scrollLeft < gridEl.scrollWidth - gridEl.clientWidth - 1;
	}

	function scrollPage(dir: number) {
		if (!gridEl) return;
		gridEl.scrollBy({ left: dir * gridEl.clientWidth });
	}

	$effect(() => {
		if (!gridEl) return;
		updateScrollState();
		const ro = new ResizeObserver(updateScrollState);
		ro.observe(gridEl);
		return () => ro.disconnect();
	});
</script>

<div class="relative group">
	<div bind:this={gridEl} class="marketplace-grid gap-px" onscroll={updateScrollState}>
		{@render children()}
	</div>
	{#if canScrollLeft}
		<button type="button" class="marketplace-grid-prev" onclick={() => scrollPage(-1)}>
			<ChevronLeft class="w-4 h-4" />
		</button>
	{/if}
	{#if canScrollRight}
		<button type="button" class="marketplace-grid-next" onclick={() => scrollPage(1)}>
			<ChevronRight class="w-4 h-4" />
		</button>
	{/if}
</div>
