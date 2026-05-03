<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	onMount(() => {
		goto(`/agent/persona/${$page.params.name}/chat`, { replaceState: true });
	});
</script>

<div class="flex items-center justify-center h-full">
	<span class="loading loading-spinner loading-lg"></span>
</div>
