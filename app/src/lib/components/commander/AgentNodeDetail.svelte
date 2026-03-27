<script lang="ts">
	import { X, MessageSquare, Settings } from 'lucide-svelte';
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';

	let {
		node = null,
		onclose = () => {},
	} = $props<{
		node: { id: string; data: { name: string; description?: string; status?: string; workflowCount?: number } } | null;
		onclose?: () => void;
	}>();

	function openChat() {
		if (!node) return;
		goto(`/agent/role/${node.id}/chat`);
	}

	function openSettings() {
		if (!node) return;
		goto(`/agent/role/${node.id}/settings`);
	}
</script>

{#if node}
	<div class="commander-detail-panel">
		<div class="flex items-center justify-between px-4 py-3 border-b border-base-content/10">
			<h3 class="text-sm font-semibold text-base-content/80">{node.data.name}</h3>
			<button type="button" class="btn btn-xs btn-ghost btn-circle" onclick={onclose}>
				<X size={14} />
			</button>
		</div>

		<div class="px-4 py-3 space-y-3">
			{#if node.data.description}
				<p class="text-xs text-base-content/60">{node.data.description}</p>
			{/if}

			<div class="flex items-center gap-2 text-xs text-base-content/60">
				<span class="inline-block w-2 h-2 rounded-full {node.data.status === 'running' ? 'bg-amber-400' : node.data.status === 'active' ? 'bg-emerald-400' : 'bg-base-content/20'}"></span>
				{node.data.status === 'running' ? $t('common.running') : node.data.status === 'active' ? $t('common.active') : $t('common.paused')}
			</div>

			{#if node.data.workflowCount}
				<div class="text-xs text-base-content/60">
					{$t('commander.workflowCount', { values: { count: node.data.workflowCount } })}
				</div>
			{/if}

			<div class="flex gap-2 pt-2">
				<button type="button" class="btn btn-sm btn-primary flex-1" onclick={openChat}>
					<MessageSquare size={14} />
					{$t('agent.chatTab')}
				</button>
				<button type="button" class="btn btn-sm btn-ghost flex-1" onclick={openSettings}>
					<Settings size={14} />
					{$t('commander.settings')}
				</button>
			</div>
		</div>
	</div>
{/if}
