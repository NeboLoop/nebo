<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	onMount(() => {
		goto('/settings/profile', { replaceState: true });
	});
</script>

<div class="flex items-center justify-center py-12">
	<p class="text-base-content/40">Redirecting...</p>
</div>
