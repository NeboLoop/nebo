<script lang="ts">
	import { getContext } from 'svelte';
	import AutomationsSection from '$lib/components/agent/AutomationsSection.svelte';
	import { t } from 'svelte-i18n';

	const channelState = getContext<{
		activeAgentId: string;
		activeAgentName: string;
	}>('channelState');
</script>

<svelte:head>
	<title>Nebo - {channelState.activeAgentName || $t('agent.automate')} - {$t('agent.automate')}</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 overflow-y-auto">
		<div class="max-w-3xl mx-auto px-6 py-6">
			{#if channelState.activeAgentId}
				<AutomationsSection
					entityType="agent"
					entityId={channelState.activeAgentId}
					roleId={channelState.activeAgentId}
				/>
			{:else}
				<div class="flex justify-center py-12">
					<span class="loading loading-spinner loading-md text-primary"></span>
				</div>
			{/if}
		</div>
	</div>
</div>
