<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { auth } from '$lib/stores/auth';

	let { children }: { children: Snippet } = $props();

	onMount(() => {
		const authState = get(auth);
		const userId = authState.user?.id || 'anonymous';
		const wsClient = getWebSocketClient();
		wsClient.connect(userId);
	});
</script>

<svelte:head>
	<title>Developer Window - Nebo</title>
</svelte:head>

<div class="h-dvh flex flex-col overflow-hidden bg-base-100">
	{@render children()}
</div>
