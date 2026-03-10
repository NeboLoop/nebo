<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	// Redirect to profile tab by default
	onMount(() => {
		goto('/settings/profile', { replaceState: true });
	});
</script>

<div class="flex items-center justify-center py-12">
	<p class="text-base-400">Redirecting...</p>
</div>
