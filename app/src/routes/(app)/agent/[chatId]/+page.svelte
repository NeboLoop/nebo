<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	// Single-bot paradigm: all chat routes redirect to the main agent page
	onMount(() => {
		goto('/agent', { replaceState: true });
	});
</script>

<div class="flex items-center justify-center h-full">
	<span class="loading loading-spinner loading-lg"></span>
</div>
