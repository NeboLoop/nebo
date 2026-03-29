<script lang="ts">
	import { goto } from '$app/navigation';
	import InstallCode from '$lib/components/InstallCode.svelte';
	import PricePill from './PricePill.svelte';
	import ArtifactIcon from './ArtifactIcon.svelte';
	import AgentSetupModal from '$lib/components/agent-setup/AgentSetupModal.svelte';
	import { type AppItem, itemHref } from '$lib/types/marketplace';
	import { installStoreProduct, listRoles as listAgents, activateRole as activateAgent } from '$lib/api/nebo';
	import webapi from '$lib/api/gocliRequest';
	import { CheckCircle } from 'lucide-svelte';

	let { item, label }: { item: AppItem; label?: string } = $props();

	const typeLabel = label || (item.type === 'agent' ? 'AGENT' : item.type === 'workflow' ? 'WORKFLOW' : 'SKILL');

	let installing = $state(false);
	let showSetupModal = $state(false);
	let setupInputs = $state<Record<string, unknown>>({});

	async function handleGetClick(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		if (installing || item.installed) return;

		installing = true;
		try {
			if (item.type === 'agent') {
				// Fetch agent detail to check for inputs
				const detail = await webapi.get<any>(`/api/v1/store/products/${item.id}`).catch(() => null);
				const inputs = detail?.typeConfig?.inputs || detail?.inputs || {};

				if (Object.keys(inputs).length > 0) {
					setupInputs = inputs;
					showSetupModal = true;
					installing = false;
					return;
				}
			}

			// No inputs or not an agent — install directly
			await installStoreProduct(item.id);

			if (item.type === 'agent') {
				// Find and activate the agent
				const agentsRes = await listAgents();
				const allAgents = agentsRes?.roles || [];
				const matched = allAgents.find(
					(r: any) => r.name?.toLowerCase() === item.name.toLowerCase()
				);

				if (matched) {
					await activateAgent(matched.id);
					goto(`/agent/persona/${matched.id}/chat`);
					return;
				}
			}

			// For non-agent items, just mark installed
			item.installed = true;
		} catch {
			// ignore
		} finally {
			installing = false;
		}
	}

	function handleSetupComplete(agentId: string) {
		showSetupModal = false;
		goto(`/agent/persona/${agentId}/chat`);
	}

	function handleSetupCancel() {
		showSetupModal = false;
	}
</script>

<a
	href={itemHref(item)}
	class="rounded-2xl overflow-hidden border border-base-content/10 hover:border-base-content/40 transition-colors"
>
	<div class="p-5">
		<p class="text-sm font-semibold text-primary uppercase tracking-wider mb-2">{typeLabel}</p>
		<div class="flex items-start gap-4">
			<ArtifactIcon emoji={item.iconEmoji} bg={item.iconBg} size="xl" />
			<div class="flex-1 min-w-0">
				<h3 class="font-display text-lg font-bold leading-tight">{item.name}</h3>
				{#if item.description}
				<p class="text-sm text-base-content/80 mt-1 line-clamp-2 leading-relaxed">{item.description}</p>
			{/if}
				<div class="flex items-center gap-1.5 mt-2">
					{#if item.author}
						<span class="text-sm text-base-content/80 truncate">{item.author}</span>
						{#if item.authorVerified}
							<CheckCircle class="w-3 h-3 text-info shrink-0" />
						{/if}
					{/if}
				</div>
			</div>
			<button
				type="button"
				class="shrink-0"
				onclick={handleGetClick}
				disabled={installing}
			>
				{#if installing}
					<span class="btn-market btn-market-get">
						<span class="loading loading-spinner loading-xs"></span>
					</span>
				{:else}
					<PricePill price={item.price} installed={item.installed} />
				{/if}
			</button>
		</div>
		<div class="flex items-center justify-between mt-3">
			<InstallCode code={item.code} inline />
		</div>
	</div>
</a>

{#if showSetupModal}
	<AgentSetupModal
		appId={item.id}
		agentName={item.name}
		agentDescription={item.description}
		inputs={setupInputs}
		onComplete={handleSetupComplete}
		onCancel={handleSetupCancel}
	/>
{/if}
