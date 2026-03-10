<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { auth } from '$lib/stores/auth';
	import { get } from 'svelte/store';

	let { children }: { children: Snippet } = $props();

	onMount(() => {
		// Connect WebSocket — pass token from auth store (in-memory)
		const wsClient = getWebSocketClient();
		const authState = get(auth);
		wsClient.connect(authState.token ?? undefined);
	});
</script>

<svelte:head>
	<title>Developer Window - Nebo</title>
</svelte:head>

<div class="h-dvh flex flex-col overflow-hidden bg-base-100">
	{@render children()}
</div>
