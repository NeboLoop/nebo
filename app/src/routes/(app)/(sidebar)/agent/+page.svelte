<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	// /agent is no longer a destination — redirect to /agents overview
	onMount(() => {
		goto('/agents', { replaceState: true });
	});
</script>

<div class="flex items-center justify-center h-full">
	<span class="loading loading-spinner loading-lg"></span>
</div>
