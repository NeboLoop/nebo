<script lang="ts">
	import type { Snippet } from 'svelte';
	import { History } from 'lucide-svelte';

	let { children }: { children: Snippet } = $props();
</script>

<div class="flex flex-1 min-h-0">
	<!-- Main Content - Full Width in Companion Mode -->
	<main class="flex-1 flex flex-col min-w-0 overflow-hidden relative">
		<!-- History button in top-right corner -->
		<div class="absolute top-3 right-3 z-10">
			<a
				href="/agent/history"
				class="btn btn-sm btn-ghost gap-2"
				title="View history"
			>
				<History class="w-4 h-4" />
				<span class="hidden sm:inline">History</span>
			</a>
		</div>
		{@render children()}
	</main>
</div>
