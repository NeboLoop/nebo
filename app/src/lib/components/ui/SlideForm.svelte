<script lang="ts">
	import type { Snippet } from 'svelte';
	import { slide } from 'svelte/transition';
	import { X } from 'lucide-svelte';

	interface Props {
		show: boolean;
		title?: string;
		children: Snippet;
		footer?: Snippet;
		onclose?: () => void;
	}

	let { show = $bindable(), title, children, footer, onclose }: Props = $props();

	function handleClose() {
		show = false;
		onclose?.();
	}
</script>

{#if show}
	<div class="card bg-base-100 border border-base-300 mb-4 overflow-hidden" transition:slide={{ duration: 200 }}>
		<div class="flex items-center justify-between px-4 py-3 border-b border-base-300 bg-base-200">
			{#if title}
				<h3 class="text-sm font-semibold">{title}</h3>
			{/if}
			<button onclick={handleClose} class="btn btn-ghost btn-xs btn-circle" aria-label="Close">
				<X class="h-4 w-4" />
			</button>
		</div>
		<div class="p-4">
			{@render children()}
		</div>
		{#if footer}
			<div class="px-4 py-3 border-t border-base-300 bg-base-200 flex justify-end gap-3">
				{@render footer()}
			</div>
		{/if}
	</div>
{/if}
