<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { AppNav } from '$lib/components/navigation';
	import { getWebSocketClient } from '$lib/websocket/client';

	let { children }: { children: Snippet } = $props();

	// Full-height pages that need flex layout
	const isFullHeightRoute = $derived(
		$page.url.pathname.startsWith('/agent') ||
		$page.url.pathname.startsWith('/settings/heartbeat')
	);

	onMount(async () => {
		getWebSocketClient().connect();
	});
</script>

{#if isFullHeightRoute}
	<div class="h-dvh flex flex-col overflow-hidden bg-base-100">
		<AppNav />
		<main id="main-content" class="flex-1 flex flex-col min-h-0 overflow-hidden p-6">
			<div class="max-w-[1400px] mx-auto w-full flex-1 flex flex-col min-h-0">
				{@render children()}
			</div>
		</main>
	</div>
{:else}
	<div class="layout-app h-full">
		<AppNav />
		<main id="main-content" class="flex-1 p-6">
			<div class="max-w-[1400px] mx-auto">
				{@render children()}
			</div>
		</main>
	</div>
{/if}
