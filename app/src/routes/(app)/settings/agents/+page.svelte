<script lang="ts">
	import { onMount } from 'svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { Bot, RefreshCw, ExternalLink } from 'lucide-svelte';
	import { listAgents } from '$lib/api/nebo';
	import type { InstalledAgent } from '$lib/api/neboComponents';
	import { t } from 'svelte-i18n';

	interface FsAgent {
		name: string;
		description: string;
		source: string;
		version: string;
		isEnabled: boolean;
	}

	let agents = $state<InstalledAgent[]>([]);
	let fsAgents = $state<FsAgent[]>([]);
	let isLoading = $state(true);

	onMount(async () => {
		await loadAgents();
	});

	async function loadAgents() {
		isLoading = true;
		try {
			const resp = await listAgents();
			agents = resp.agents || [];
			fsAgents = resp.filesystemAgents || [];
		} catch (error) {
			console.error('Failed to load agents:', error);
		} finally {
			isLoading = false;
		}
	}

	function getInitial(name: string): string {
		return (name || '?').charAt(0).toUpperCase();
	}
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsAgents.title')}</h2>
		<p class="text-base text-base-content/80">{$t('settingsAgents.description')}</p>
	</div>
	<button
		class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
		onclick={loadAgents}
	>
		<RefreshCw class="w-3.5 h-3.5" />
		{$t('common.refresh')}
	</button>
</div>

{#if isLoading}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
		<Spinner class="w-5 h-5 mx-auto mb-2" />
		<p class="text-base">{$t('settingsAgents.loading')}</p>
	</div>
{:else if agents.length === 0 && fsAgents.length === 0}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-12 text-center text-base-content/90">
		<Bot class="w-12 h-12 mx-auto mb-4 opacity-20" />
		<p class="font-medium mb-2">{$t('settingsAgents.noAgents')}</p>
		<p class="text-base">{$t('settingsAgents.noAgentsHint')}</p>
	</div>
{:else}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
		{#each agents as agent}
			<div class="flex items-center gap-4 p-4">
				<div class="w-11 h-11 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
					<span class="text-lg font-bold text-primary">{getInitial(agent.name)}</span>
				</div>
				<div class="flex-1 min-w-0">
					<div class="flex items-center gap-2 mb-0.5">
						<h3 class="font-display font-bold text-base text-base-content">{agent.name}</h3>
						<span class="text-sm font-medium px-1.5 py-0.5 rounded bg-base-content/10 text-base-content/60">
							{agent.source === 'user' ? $t('settingsAgents.user') : $t('settingsAgents.installed')}
						</span>
					</div>
					{#if agent.description}
						<p class="text-base text-base-content/80 truncate">{agent.description}</p>
					{/if}
				</div>
				<a
					href="/agent/persona/{agent.id}/configure"
					class="h-7 px-2.5 rounded-md bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1"
				>
					<ExternalLink class="w-3 h-3" />
					{$t('settingsAgents.configure')}
				</a>
			</div>
		{/each}
		{#each fsAgents as agent}
			<div class="flex items-center gap-4 p-4">
				<div class="w-11 h-11 rounded-xl bg-secondary/10 flex items-center justify-center shrink-0">
					<span class="text-lg font-bold text-secondary">{getInitial(agent.name)}</span>
				</div>
				<div class="flex-1 min-w-0">
					<div class="flex items-center gap-2 mb-0.5">
						<h3 class="font-display font-bold text-base text-base-content">{agent.name}</h3>
						{#if agent.version}
							<span class="text-sm font-medium px-1.5 py-0.5 rounded bg-base-content/10 text-base-content/60">v{agent.version}</span>
						{/if}
						<span class="text-sm font-medium px-1.5 py-0.5 rounded bg-secondary/10 text-secondary">
							{agent.source}
						</span>
					</div>
					{#if agent.description}
						<p class="text-base text-base-content/80 truncate">{agent.description}</p>
					{/if}
				</div>
			</div>
		{/each}
	</div>
{/if}
