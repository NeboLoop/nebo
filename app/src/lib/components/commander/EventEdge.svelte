<script lang="ts">
	import { BaseEdge, getBezierPath } from '@xyflow/svelte';

	let {
		id,
		sourceX,
		sourceY,
		targetX,
		targetY,
		sourcePosition,
		targetPosition,
		data,
		markerEnd,
		style,
		selected,
	} = $props<{
		id: string;
		sourceX: number;
		sourceY: number;
		targetX: number;
		targetY: number;
		sourcePosition: any;
		targetPosition: any;
		data?: { label?: string; edgeType?: string };
		markerEnd?: string;
		style?: string;
		selected?: boolean;
	}>();

	const pathResult = $derived(
		getBezierPath({
			sourceX,
			sourceY,
			targetX,
			targetY,
			sourcePosition,
			targetPosition,
		})
	);

	const edgePath = $derived(pathResult[0]);
</script>

<BaseEdge
	{id}
	path={edgePath}
	{markerEnd}
	style={selected ? 'stroke: #ef4444; stroke-width: 3;' : style}
/>
