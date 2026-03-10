<script lang="ts">
	import { onMount } from 'svelte';

	interface SankeyNode {
		id: string;
		label: string;
		color?: string;
	}

	interface SankeyLink {
		source: string;
		target: string;
		value: number;
	}

	interface Props {
		nodes: SankeyNode[];
		links: SankeyLink[];
		height?: number;
		nodeWidth?: number;
		nodePadding?: number;
	}

	let {
		nodes = [],
		links = [],
		height,
		nodeWidth = 20,
		nodePadding = 10
	}: Props = $props();

	let containerRef: HTMLDivElement;
	let containerWidth = $state(600);
	let containerHeight = $state(400);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					containerWidth = Math.max(entry.contentRect.width, 300);
					if (!height) {
						containerHeight = Math.max(entry.contentRect.height, 300);
					}
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	const effectiveHeight = $derived(height || containerHeight);
	const padding = { top: 20, right: 100, bottom: 20, left: 100 };
	const chartWidth = $derived(containerWidth - padding.left - padding.right);
	const chartHeight = $derived(effectiveHeight - padding.top - padding.bottom);

	const defaultColors = [
		'rgba(99, 102, 241, 0.7)',
		'rgba(168, 85, 247, 0.7)',
		'rgba(236, 72, 153, 0.7)',
		'rgba(251, 146, 60, 0.7)',
		'rgba(34, 197, 94, 0.7)',
		'rgba(14, 165, 233, 0.7)',
	];

	// Compute node layers and positions
	const layout = $derived.by(() => {
		if (nodes.length === 0 || links.length === 0) return { nodes: [], links: [] };

		// Build adjacency for layer calculation
		const outgoing = new Map<string, string[]>();
		const incoming = new Map<string, string[]>();

		for (const link of links) {
			if (!outgoing.has(link.source)) outgoing.set(link.source, []);
			if (!incoming.has(link.target)) incoming.set(link.target, []);
			outgoing.get(link.source)!.push(link.target);
			incoming.get(link.target)!.push(link.source);
		}

		// Assign layers using topological sort
		const layers = new Map<string, number>();

		// Find source nodes (no incoming)
		const sources = nodes.filter(n => !incoming.has(n.id) || incoming.get(n.id)!.length === 0);
		for (const source of sources) {
			layers.set(source.id, 0);
		}

		// BFS to assign layers
		const queue = [...sources.map(s => s.id)];
		while (queue.length > 0) {
			const current = queue.shift()!;
			const currentLayer = layers.get(current) || 0;

			for (const target of outgoing.get(current) || []) {
				const existingLayer = layers.get(target);
				if (existingLayer === undefined || existingLayer <= currentLayer) {
					layers.set(target, currentLayer + 1);
					queue.push(target);
				}
			}
		}

		// Calculate node values (sum of incoming or outgoing, whichever is larger)
		const nodeValues = new Map<string, number>();
		for (const node of nodes) {
			let inValue = 0;
			let outValue = 0;
			for (const link of links) {
				if (link.target === node.id) inValue += link.value;
				if (link.source === node.id) outValue += link.value;
			}
			nodeValues.set(node.id, Math.max(inValue, outValue) || 1);
		}

		// Group nodes by layer
		const layerGroups = new Map<number, SankeyNode[]>();
		const maxLayer = Math.max(...Array.from(layers.values()));

		for (const node of nodes) {
			const layer = layers.get(node.id) || 0;
			if (!layerGroups.has(layer)) layerGroups.set(layer, []);
			layerGroups.get(layer)!.push(node);
		}

		// Calculate positions
		const nodePositions = new Map<string, { x: number; y: number; height: number }>();
		const layerWidth = chartWidth / (maxLayer + 1);

		for (let layer = 0; layer <= maxLayer; layer++) {
			const layerNodes = layerGroups.get(layer) || [];
			const totalValue = layerNodes.reduce((sum, n) => sum + (nodeValues.get(n.id) || 0), 0);
			const availableHeight = chartHeight - nodePadding * (layerNodes.length - 1);

			let currentY = 0;
			for (const node of layerNodes) {
				const value = nodeValues.get(node.id) || 0;
				const nodeHeight = (value / totalValue) * availableHeight;

				nodePositions.set(node.id, {
					x: layer * layerWidth,
					y: currentY,
					height: nodeHeight
				});

				currentY += nodeHeight + nodePadding;
			}
		}

		// Build positioned nodes
		const positionedNodes = nodes.map((node, i) => ({
			...node,
			position: nodePositions.get(node.id)!,
			value: nodeValues.get(node.id) || 0,
			color: node.color || defaultColors[i % defaultColors.length]
		}));

		// Build link paths with proper vertical positioning
		const linkOffsets = new Map<string, { source: number; target: number }>();
		for (const node of nodes) {
			linkOffsets.set(node.id, { source: 0, target: 0 });
		}

		const positionedLinks = links.map(link => {
			const sourceNode = positionedNodes.find(n => n.id === link.source);
			const targetNode = positionedNodes.find(n => n.id === link.target);

			if (!sourceNode || !targetNode) return null;

			const sourceValue = nodeValues.get(link.source) || 1;
			const targetValue = nodeValues.get(link.target) || 1;

			const linkHeight = (link.value / sourceValue) * sourceNode.position.height;
			const targetLinkHeight = (link.value / targetValue) * targetNode.position.height;

			const sourceOffset = linkOffsets.get(link.source)!;
			const targetOffset = linkOffsets.get(link.target)!;

			const sourceY = sourceNode.position.y + sourceOffset.source;
			const targetY = targetNode.position.y + targetOffset.target;

			sourceOffset.source += linkHeight;
			targetOffset.target += targetLinkHeight;

			// Create bezier curve path
			const sourceX = sourceNode.position.x + nodeWidth;
			const targetX = targetNode.position.x;
			const midX = (sourceX + targetX) / 2;

			const path = `
				M ${sourceX} ${sourceY}
				C ${midX} ${sourceY}, ${midX} ${targetY}, ${targetX} ${targetY}
				L ${targetX} ${targetY + targetLinkHeight}
				C ${midX} ${targetY + targetLinkHeight}, ${midX} ${sourceY + linkHeight}, ${sourceX} ${sourceY + linkHeight}
				Z
			`;

			return {
				...link,
				path,
				color: sourceNode.color.replace('0.7', '0.3')
			};
		}).filter((link): link is NonNullable<typeof link> => link !== null);

		return { nodes: positionedNodes, links: positionedLinks };
	});

	function formatValue(value: number): string {
		if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toLocaleString();
	}
</script>

<div bind:this={containerRef} class="sankey-chart-container w-full h-full">
	{#if nodes.length === 0 || links.length === 0}
		<div class="empty-state flex items-center justify-center h-full">
			<p class="text-base-content/60 text-sm">No data to display</p>
		</div>
	{:else}
		<svg width={containerWidth} height={effectiveHeight}>
			<g transform="translate({padding.left}, {padding.top})">
				<!-- Links -->
				{#each layout.links as link}
					<path
						d={link.path}
						fill={link.color}
						class="transition-all duration-200 hover:opacity-80 cursor-pointer"
					>
						<title>{link.source} → {link.target}: {formatValue(link.value)}</title>
					</path>
				{/each}

				<!-- Nodes -->
				{#each layout.nodes as node}
					<g>
						<rect
							x={node.position.x}
							y={node.position.y}
							width={nodeWidth}
							height={node.position.height}
							fill={node.color}
							stroke="#1e293b"
							stroke-width="1"
							rx="2"
							class="transition-all duration-200 hover:opacity-80 cursor-pointer"
						>
							<title>{node.label}: {formatValue(node.value)}</title>
						</rect>

						<!-- Node label -->
						<text
							x={node.position.x < chartWidth / 2 ? node.position.x - 5 : node.position.x + nodeWidth + 5}
							y={node.position.y + node.position.height / 2}
							text-anchor={node.position.x < chartWidth / 2 ? 'end' : 'start'}
							dominant-baseline="middle"
							class="text-xs fill-base-content/70"
						>
							{node.label}
						</text>
					</g>
				{/each}
			</g>
		</svg>
	{/if}
</div>
