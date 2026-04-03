<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	onMount(() => {
		goto('/agent/assistant/chat', { replaceState: true });
	});
</script>
