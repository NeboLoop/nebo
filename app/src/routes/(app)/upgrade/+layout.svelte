<script lang="ts">
	import type { Snippet } from 'svelte';
	import { goto } from '$app/navigation';
	import { ArrowLeft, X } from 'lucide-svelte';

	let { children }: { children: Snippet } = $props();

	function close() {
		goto('/settings/billing');
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') close();
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="fixed inset-0 z-[70] flex flex-col bg-base-100">
	<!-- Top bar -->
	<div class="shrink-0 flex items-center justify-between px-6 py-3 border-b border-base-content/10">
		<button
			onclick={close}
			class="flex items-center gap-2 text-base text-base-content/80 hover:text-base-content transition-colors"
		>
			<ArrowLeft class="w-4 h-4" />
			<span class="font-medium">Back to Billing</span>
		</button>
		<button
			onclick={close}
			class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors"
			aria-label="Close"
		>
			<X class="w-4 h-4 text-base-content/90" />
		</button>
	</div>

	<!-- Scrollable content -->
	<div class="flex-1 overflow-y-auto">
		<div class="max-w-5xl mx-auto px-6 py-10">
			{@render children()}
		</div>
	</div>
</div>
