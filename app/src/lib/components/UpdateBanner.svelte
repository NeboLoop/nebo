<script lang="ts">
	import { updateInfo, updateDismissed } from '$lib/stores/update';
	import { ArrowUpCircle, X } from 'lucide-svelte';

	let show = $derived(
		$updateInfo?.available && !$updateDismissed
	);

	function dismiss() {
		updateDismissed.set(true);
	}
</script>

{#if show && $updateInfo}
	<div class="alert alert-info shadow-lg mx-4 mt-2 mb-0 flex items-center gap-3 py-2 px-4 text-sm">
		<ArrowUpCircle class="w-5 h-5 shrink-0" />
		<div class="flex-1 min-w-0">
			<span class="font-semibold">Nebo {$updateInfo.latest_version}</span> is available
			<span class="text-info-content/60">(you're on {$updateInfo.current_version})</span>
		</div>
		<a
			href={$updateInfo.release_url}
			target="_blank"
			rel="noopener noreferrer"
			class="btn btn-sm btn-ghost"
		>
			Release notes
		</a>
		<button class="btn btn-sm btn-ghost btn-square" onclick={dismiss} aria-label="Dismiss">
			<X class="w-4 h-4" />
		</button>
	</div>
{/if}
