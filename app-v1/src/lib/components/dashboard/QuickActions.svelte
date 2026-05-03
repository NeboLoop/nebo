<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { Plus, Store, Settings } from 'lucide-svelte';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { updateInfo } from '$lib/stores/update';

	let { mcpCount = 0 }: { mcpCount: number } = $props();

	let wsStatus = $state<string>('connecting');

	const ws = getWebSocketClient();
	let unsub: (() => void) | undefined;

	onMount(() => {
		unsub = ws.onStatus((status) => {
			wsStatus = status;
		});
	});

	onDestroy(() => {
		unsub?.();
	});

	let wsConnected = $derived(wsStatus === 'connected');
</script>

<div class="flex flex-col gap-6">
	<!-- Quick Actions -->
	<div>
		<div class="dashboard-section-title">Quick Actions</div>
		<div class="flex flex-col gap-2">
			<button class="btn btn-outline btn-sm justify-start gap-2" onclick={() => goto('/agent/assistant/chat')}>
				<Plus class="w-4 h-4" /> New Chat
			</button>
			<button class="btn btn-outline btn-sm justify-start gap-2" onclick={() => goto('/marketplace')}>
				<Store class="w-4 h-4" /> Browse Marketplace
			</button>
			<button class="btn btn-outline btn-sm justify-start gap-2" onclick={() => goto('/settings')}>
				<Settings class="w-4 h-4" /> Settings
			</button>
		</div>
	</div>

	<!-- System Status -->
	<div>
		<div class="dashboard-section-title">System Status</div>
		<div class="card bg-base-200 border border-base-300">
			<div class="card-body p-4 gap-3">
				<div class="flex items-center gap-2 text-base">
					<span class="dashboard-status-dot {wsConnected ? 'bg-success' : 'bg-error'}"></span>
					<span class="text-base-content/90">WebSocket</span>
					<span class="text-sm text-base-content/80 ml-auto">{wsConnected ? 'Connected' : 'Disconnected'}</span>
				</div>
				<div class="flex items-center gap-2 text-base">
					<span class="dashboard-status-dot bg-info"></span>
					<span class="text-base-content/90">MCP Servers</span>
					<span class="text-sm text-base-content/80 ml-auto">{mcpCount}</span>
				</div>
				{#if $updateInfo?.available}
					<div class="flex items-center gap-2 text-base">
						<span class="dashboard-status-dot bg-warning"></span>
						<span class="text-base-content/90">Update</span>
						<span class="text-sm text-base-content/80 ml-auto">v{$updateInfo.latestVersion}</span>
					</div>
				{/if}
			</div>
		</div>
	</div>
</div>
