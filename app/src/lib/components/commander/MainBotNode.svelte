<script lang="ts">
	import { Handle, Position } from '@xyflow/svelte';

	let { data } = $props<{ data: { name: string } }>();
</script>

<div class="commander-main-node">
	<Handle type="target" position={Position.Top} isConnectable={true} />

	<div class="flex items-center gap-3 px-4 py-3">
		<div class="w-10 h-10 rounded-full flex items-center justify-center text-base font-bold bg-primary/10 text-primary ring-2 ring-primary/30 shrink-0">
			N
		</div>
		<div class="min-w-0">
			<div class="text-sm font-semibold text-base-content/90">{data.name}</div>
			<div class="text-xs text-base-content/50">Primary Coordinator</div>
		</div>
	</div>

	<Handle type="source" position={Position.Bottom} isConnectable={true} />
</div>
