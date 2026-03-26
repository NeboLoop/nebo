<script lang="ts">
	import { Handle, Position } from '@xyflow/svelte';

	const BOT_COLORS = [
		{ bg: 'bg-blue-500/10', text: 'text-blue-500' },
		{ bg: 'bg-violet-500/10', text: 'text-violet-500' },
		{ bg: 'bg-emerald-500/10', text: 'text-emerald-500' },
		{ bg: 'bg-amber-500/10', text: 'text-amber-500' },
		{ bg: 'bg-rose-500/10', text: 'text-rose-500' },
		{ bg: 'bg-cyan-500/10', text: 'text-cyan-500' },
	];

	function nameHash(name: string): number {
		let hash = 0;
		for (let i = 0; i < name.length; i++) {
			hash = ((hash << 5) - hash + name.charCodeAt(i)) | 0;
		}
		return Math.abs(hash);
	}

	function getColor(name: string) {
		return BOT_COLORS[nameHash(name) % BOT_COLORS.length];
	}

	let { data } = $props<{ data: { name: string; description?: string; status?: string; workflowCount?: number } }>();

	const color = $derived(getColor(data.name));
	const initial = $derived(data.name.charAt(0).toUpperCase());
	const isRunning = $derived(data.status === 'running');
</script>

<div class="commander-node" class:commander-node--running={isRunning}>
	<Handle type="target" position={Position.Top} isConnectable={true} />

	<div class="flex items-center gap-2.5 px-3 py-2.5">
		<div class="w-8 h-8 rounded-full flex items-center justify-center text-sm font-semibold {color.bg} {color.text} shrink-0">
			{initial}
		</div>
		<div class="min-w-0">
			<div class="text-sm font-medium text-base-content/80 truncate">{data.name}</div>
			{#if data.workflowCount}
				<div class="text-xs text-base-content/50">{data.workflowCount} workflow{data.workflowCount !== 1 ? 's' : ''}</div>
			{/if}
		</div>
		<div class="ml-auto shrink-0">
			{#if isRunning}
				<div class="w-2.5 h-2.5 rounded-full bg-amber-400 commander-pulse"></div>
			{:else if data.status === 'active'}
				<div class="w-2.5 h-2.5 rounded-full bg-emerald-400"></div>
			{:else}
				<div class="w-2.5 h-2.5 rounded-full bg-base-content/20"></div>
			{/if}
		</div>
	</div>

	<Handle type="source" position={Position.Bottom} isConnectable={true} />
</div>
