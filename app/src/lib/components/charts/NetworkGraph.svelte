<script lang="ts">
	import { onMount } from 'svelte';

	interface NetworkNode {
		id: string;
		label: string;
		size?: number;
		color?: string;
		group?: string;
	}

	interface NetworkEdge {
		source: string;
		target: string;
		weight?: number;
		label?: string;
	}

	interface Props {
		nodes: NetworkNode[];
		edges: NetworkEdge[];
		height?: number;
		directed?: boolean;
		showLabels?: boolean;
	}

	let {
		nodes = [],
		edges = [],
		height,
		directed = false,
		showLabels = true
	}: Props = $props();

	let containerRef: HTMLDivElement;
	let containerWidth = $state(500);
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
	const padding = 50;

	const defaultColors = [
		'rgba(99, 102, 241, 0.85)',
		'rgba(168, 85, 247, 0.85)',
		'rgba(236, 72, 153, 0.85)',
		'rgba(251, 146, 60, 0.85)',
		'rgba(34, 197, 94, 0.85)',
		'rgba(14, 165, 233, 0.85)',
	];

	const groupColors = new Map<string, string>();

	// Simple force-directed layout simulation
	const nodePositions = $derived.by(() => {
		if (nodes.length === 0) return new Map<string, { x: number; y: number }>();

		const positions = new Map<string, { x: number; y: number }>();
		const width = containerWidth - 2 * padding;
		const height = effectiveHeight - 2 * padding;

		// Initialize positions in a circle
		nodes.forEach((node, i) => {
			const angle = (2 * Math.PI * i) / nodes.length;
			const r = Math.min(width, height) * 0.35;
			positions.set(node.id, {
				x: width / 2 + r * Math.cos(angle),
				y: height / 2 + r * Math.sin(angle)
			});
		});

		// Simple force simulation (run for fixed iterations)
		const iterations = 50;
		const repulsion = 5000;
		const attraction = 0.05;
		const centerPull = 0.01;

		for (let iter = 0; iter < iterations; iter++) {
			const forces = new Map<string, { fx: number; fy: number }>();
			nodes.forEach(n => forces.set(n.id, { fx: 0, fy: 0 }));

			// Repulsion between all nodes
			for (let i = 0; i < nodes.length; i++) {
				for (let j = i + 1; j < nodes.length; j++) {
					const n1 = nodes[i];
					const n2 = nodes[j];
					const p1 = positions.get(n1.id)!;
					const p2 = positions.get(n2.id)!;

					const dx = p2.x - p1.x;
					const dy = p2.y - p1.y;
					const dist = Math.sqrt(dx * dx + dy * dy) || 1;

					const force = repulsion / (dist * dist);
					const fx = (dx / dist) * force;
					const fy = (dy / dist) * force;

					forces.get(n1.id)!.fx -= fx;
					forces.get(n1.id)!.fy -= fy;
					forces.get(n2.id)!.fx += fx;
					forces.get(n2.id)!.fy += fy;
				}
			}

			// Attraction along edges
			for (const edge of edges) {
				const p1 = positions.get(edge.source);
				const p2 = positions.get(edge.target);
				if (!p1 || !p2) continue;

				const dx = p2.x - p1.x;
				const dy = p2.y - p1.y;
				const dist = Math.sqrt(dx * dx + dy * dy) || 1;

				const force = dist * attraction;
				const fx = (dx / dist) * force;
				const fy = (dy / dist) * force;

				forces.get(edge.source)!.fx += fx;
				forces.get(edge.source)!.fy += fy;
				forces.get(edge.target)!.fx -= fx;
				forces.get(edge.target)!.fy -= fy;
			}

			// Center pull
			nodes.forEach(node => {
				const pos = positions.get(node.id)!;
				const force = forces.get(node.id)!;
				force.fx += (width / 2 - pos.x) * centerPull;
				force.fy += (height / 2 - pos.y) * centerPull;
			});

			// Apply forces with damping
			const damping = 0.8 - (iter / iterations) * 0.5;
			nodes.forEach(node => {
				const pos = positions.get(node.id)!;
				const force = forces.get(node.id)!;
				pos.x += force.fx * damping;
				pos.y += force.fy * damping;

				// Keep within bounds
				pos.x = Math.max(30, Math.min(width - 30, pos.x));
				pos.y = Math.max(30, Math.min(height - 30, pos.y));
			});
		}

		return positions;
	});

	function getNodeColor(node: NetworkNode, index: number): string {
		if (node.color) return node.color;
		if (node.group) {
			if (!groupColors.has(node.group)) {
				groupColors.set(node.group, defaultColors[groupColors.size % defaultColors.length]);
			}
			return groupColors.get(node.group)!;
		}
		return defaultColors[index % defaultColors.length];
	}

	function getNodeSize(node: NetworkNode): number {
		return node.size || 12;
	}

	function getEdgePath(edge: NetworkEdge): string | null {
		const sourcePos = nodePositions.get(edge.source);
		const targetPos = nodePositions.get(edge.target);
		if (!sourcePos || !targetPos) return null;

		return `M ${sourcePos.x + padding} ${sourcePos.y + padding} L ${targetPos.x + padding} ${targetPos.y + padding}`;
	}

	function getEdgeMidpoint(edge: NetworkEdge): { x: number; y: number } | null {
		const sourcePos = nodePositions.get(edge.source);
		const targetPos = nodePositions.get(edge.target);
		if (!sourcePos || !targetPos) return null;

		return {
			x: (sourcePos.x + targetPos.x) / 2 + padding,
			y: (sourcePos.y + targetPos.y) / 2 + padding
		};
	}

	// For directed edges, calculate arrow position
	function getArrowTransform(edge: NetworkEdge): string | null {
		const sourcePos = nodePositions.get(edge.source);
		const targetPos = nodePositions.get(edge.target);
		if (!sourcePos || !targetPos) return null;

		const targetNode = nodes.find(n => n.id === edge.target);
		const nodeRadius = targetNode ? getNodeSize(targetNode) + 2 : 12;

		const dx = targetPos.x - sourcePos.x;
		const dy = targetPos.y - sourcePos.y;
		const dist = Math.sqrt(dx * dx + dy * dy);
		if (dist === 0) return null;

		const angle = Math.atan2(dy, dx) * (180 / Math.PI);
		const x = targetPos.x + padding - (dx / dist) * nodeRadius;
		const y = targetPos.y + padding - (dy / dist) * nodeRadius;

		return `translate(${x}, ${y}) rotate(${angle})`;
	}
</script>

<div bind:this={containerRef} class="network-graph-container w-full h-full">
	{#if nodes.length === 0}
		<div class="empty-state flex items-center justify-center h-full">
			<p class="text-base-content/60 text-sm">No data to display</p>
		</div>
	{:else}
		<svg width={containerWidth} height={effectiveHeight}>
			<!-- Arrow marker definition -->
			{#if directed}
				<defs>
					<marker
						id="arrowhead-network"
						markerWidth="10"
						markerHeight="7"
						refX="9"
						refY="3.5"
						orient="auto"
					>
						<polygon points="0 0, 10 3.5, 0 7" fill="rgba(148, 163, 184, 0.6)" />
					</marker>
				</defs>
			{/if}

			<!-- Edges -->
			{#each edges as edge}
				{@const path = getEdgePath(edge)}
				{#if path}
					<path
						d={path}
						fill="none"
						stroke="rgba(148, 163, 184, 0.4)"
						stroke-width={edge.weight ? Math.min(edge.weight, 5) : 2}
						marker-end={directed ? 'url(#arrowhead-network)' : undefined}
						class="transition-all duration-200 hover:stroke-base-content/60"
					>
						<title>{edge.source} → {edge.target}{edge.label ? `: ${edge.label}` : ''}</title>
					</path>

					<!-- Edge label -->
					{#if edge.label}
						{@const midpoint = getEdgeMidpoint(edge)}
						{#if midpoint}
							<text
								x={midpoint.x}
								y={midpoint.y - 5}
								text-anchor="middle"
								class="text-xs fill-base-content/60"
								font-size="9"
							>
								{edge.label}
							</text>
						{/if}
					{/if}
				{/if}
			{/each}

			<!-- Nodes -->
			{#each nodes as node, i}
				{@const pos = nodePositions.get(node.id)}
				{#if pos}
					<g class="network-node">
						<circle
							cx={pos.x + padding}
							cy={pos.y + padding}
							r={getNodeSize(node)}
							fill={getNodeColor(node, i)}
							stroke="#1e293b"
							stroke-width="2"
							class="transition-all duration-200 hover:opacity-80 cursor-pointer"
						>
							<title>{node.label}</title>
						</circle>

						{#if showLabels}
							<text
								x={pos.x + padding}
								y={pos.y + padding + getNodeSize(node) + 14}
								text-anchor="middle"
								class="text-xs fill-base-content/70 pointer-events-none"
								font-size="10"
							>
								{node.label.length > 12 ? node.label.slice(0, 12) + '..' : node.label}
							</text>
						{/if}
					</g>
				{/if}
			{/each}
		</svg>
	{/if}
</div>
