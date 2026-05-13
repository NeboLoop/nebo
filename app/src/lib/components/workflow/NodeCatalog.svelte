<script lang="ts">
	import { onMount } from 'svelte';
	import { NODE_CATALOG_ITEMS } from '$lib/tokens.js';
	import { catalogTypeToActivityType, getActivityType } from '$lib/utils/workflowTypes';
	import * as nebo from '$lib/api/nebo';

	let {
		onselect,
		onclose,
	}: {
		onselect?: (item: Record<string, unknown>) => void;
		onclose?: () => void;
	} = $props();

	let search = $state('');
	let dynamicCatalog = $state(NODE_CATALOG_ITEMS);

	onMount(async () => {
		try {
			const [mcpRes, agentsRes] = await Promise.all([
				nebo.listIntegrations(),
				nebo.listAgents(),
			]);
			// Rebuild catalog with dynamic connectors and agents
			const staticCats = NODE_CATALOG_ITEMS.filter(
				c => c.category !== 'Connectors (MCP)' && c.category !== 'Agents'
			);
			const connectorItems = (mcpRes?.integrations || [])
				.filter((s) => s.isEnabled && s.connectionStatus === 'connected')
				.map((s) => ({
					type: `connector-${s.id}`,
					label: s.name,
					desc: `${s.toolCount || 0} tools available`,
					icon: '⊞',
					serverId: s.id,
					serverName: s.name,
				}));
			interface AgentRecord { id: string; name: string; role?: string; description?: string; color?: string }
			const agentItems = ((agentsRes?.agents || []) as AgentRecord[])
				.filter((a) => a.id !== 'assistant')
				.map((a) => ({
					type: `agent-${a.id}`,
					label: a.name,
					desc: a.role || a.description || '',
					icon: (a.name || '?')[0],
					agentId: a.id,
					agentColor: a.color || '',
				}));
			dynamicCatalog = [
				...staticCats.slice(0, -1), // Everything before Output
				{ category: 'Connectors (MCP)', items: connectorItems },
				{ category: 'Agents', items: agentItems },
				...staticCats.slice(-1), // Output category
			];
		} catch {}
	});

	const filtered = $derived.by(() => {
		const q = search.toLowerCase().trim();
		if (!q) return dynamicCatalog;
		return dynamicCatalog
			.map(cat => ({
				...cat,
				items: cat.items.filter(item =>
					item.label.toLowerCase().includes(q) || item.desc.toLowerCase().includes(q)
				),
			}))
			.filter(cat => cat.items.length > 0);
	});
</script>

<!-- Header -->
<div class="flex items-center justify-between px-4 py-3 border-b border-base-content/10 shrink-0">
	<div class="text-sm font-semibold">Add Node</div>
	<button
		class="w-6 h-6 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base"
		onclick={onclose}
	>&times;</button>
</div>

<!-- Search -->
<div class="px-3 py-2 border-b border-base-content/10 shrink-0">
	<input
		type="text"
		class="input input-sm input-bordered w-full"
		placeholder="Search nodes..."
		bind:value={search}
	/>
</div>

<!-- Catalog list -->
<div class="flex-1 overflow-y-auto py-2">
	{#each filtered as category}
		<div class="px-3 mb-3">
			<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{category.category}</div>
			<div class="flex flex-col gap-0.5">
				{#each category.items as item}
					{@const typeDef = getActivityType(catalogTypeToActivityType(item.type))}
					<button
						class="w-full flex items-center gap-2.5 px-2.5 py-2 rounded-lg border border-transparent text-left cursor-grab transition-colors bg-transparent hover:bg-base-200/50 hover:border-base-300"
						draggable="true"
						ondragstart={(e) => {
							e.dataTransfer?.setData('application/x-workflow-node', JSON.stringify(item));
							if (e.dataTransfer) e.dataTransfer.effectAllowed = 'copy';
						}}
						onclick={() => onselect?.(item)}
					>
						<div class="w-7 h-7 rounded-md bg-base-200 border {typeDef.accentClass} flex items-center justify-center text-sm shrink-0">{item.icon}</div>
						<div class="flex-1 min-w-0">
							<div class="text-sm font-medium truncate">{item.label}</div>
							<div class="text-xs text-base-content/60 truncate">{item.desc}</div>
						</div>
					</button>
				{/each}
			</div>
		</div>
	{/each}

	{#if filtered.length === 0}
		<div class="flex flex-col items-center justify-center py-8 text-base-content/40">
			<div class="text-2xl mb-1">&#x2205;</div>
			<div class="text-xs">No nodes match "{search}"</div>
		</div>
	{/if}
</div>
