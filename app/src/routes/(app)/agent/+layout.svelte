<script lang="ts">
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();
</script>

<div class="flex flex-1 min-h-0">
	<!-- Main Content - Full Width in Companion Mode -->
	<main class="flex-1 flex flex-col min-w-0 overflow-hidden">
		{@render children()}
	</main>
</div>
