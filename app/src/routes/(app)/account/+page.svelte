<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	// Redirect to new settings page
	onMount(() => {
		goto('/settings', { replaceState: true });
	});
</script>

<div class="flex items-center justify-center py-12">
	<p class="text-base-content/60">Redirecting to settings...</p>
</div>
