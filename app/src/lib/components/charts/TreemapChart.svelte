<script lang="ts">
	import { onMount } from 'svelte';

	interface TreemapNode {
		label: string;
		value: number;
		color?: string;
		children?: TreemapNode[];
	}

	interface Props {
		data: TreemapNode[];
		height?: number;
		showLabels?: boolean;
		showValues?: boolean;
	}

	let {
		data = [],
		height,
		showLabels = true,
		showValues = true
	}: Props = $props();

	let containerRef: HTMLDivElement;
	let containerWidth = $state(400);
	let containerHeight = $state(300);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					containerWidth = Math.max(entry.contentRect.width, 200);
					if (!height) {
						containerHeight = Math.max(entry.contentRect.height, 200);
					}
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	const effectiveHeight = $derived(height || containerHeight);

	const defaultColors = [
		'rgba(99, 102, 241, 0.85)',
		'rgba(168, 85, 247, 0.85)',
		'rgba(236, 72, 153, 0.85)',
		'rgba(251, 146, 60, 0.85)',
		'rgba(34, 197, 94, 0.85)',
		'rgba(14, 165, 233, 0.85)',
		'rgba(244, 63, 94, 0.85)',
		'rgba(139, 92, 246, 0.85)',
	];

	const totalValue = $derived(data.reduce((sum, d) => sum + d.value, 0));

	// Squarified treemap algorithm
	interface Rect {
		x: number;
		y: number;
		width: number;
		height: number;
		node: TreemapNode;
		colorIndex: number;
	}

	const rectangles = $derived.by(() => {
		if (data.length === 0 || totalValue === 0) return [];

		const rects: Rect[] = [];
		const padding = 2;

		// Sort data by value descending
		const sortedData = [...data].sort((a, b) => b.value - a.value);

		// Simple slice-and-dice algorithm
		let currentX = padding;
		let currentY = padding;
		let remainingWidth = containerWidth - padding * 2;
		let remainingHeight = effectiveHeight - padding * 2;
		let isHorizontal = remainingWidth >= remainingHeight;

		sortedData.forEach((node, index) => {
			const ratio = node.value / totalValue;

			let rectWidth: number;
			let rectHeight: number;

			if (isHorizontal) {
				rectWidth = remainingWidth * ratio / (sortedData.slice(index).reduce((s, d) => s + d.value, 0) / totalValue);
				rectHeight = remainingHeight;
			} else {
				rectWidth = remainingWidth;
				rectHeight = remainingHeight * ratio / (sortedData.slice(index).reduce((s, d) => s + d.value, 0) / totalValue);
			}

			rects.push({
				x: currentX,
				y: currentY,
				width: Math.max(rectWidth - padding, 0),
				height: Math.max(rectHeight - padding, 0),
				node,
				colorIndex: index
			});

			if (isHorizontal) {
				currentX += rectWidth;
				remainingWidth -= rectWidth;
			} else {
				currentY += rectHeight;
				remainingHeight -= rectHeight;
			}

			// Alternate direction for better aspect ratios
			if (index % 2 === 0) {
				isHorizontal = !isHorizontal;
			}
		});

		return rects;
	});

	function getColor(index: number, node: TreemapNode): string {
		return node.color || defaultColors[index % defaultColors.length];
	}

	function formatValue(value: number): string {
		if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toLocaleString();
	}

	function getTextColor(bgColor: string): string {
		// Simple heuristic: lighter backgrounds get dark text
		return '#ffffff';
	}

	function shouldShowLabel(rect: Rect): boolean {
		return rect.width > 50 && rect.height > 30;
	}

	function shouldShowValue(rect: Rect): boolean {
		return rect.width > 40 && rect.height > 50;
	}
</script>

<div bind:this={containerRef} class="treemap-chart-container w-full h-full">
	{#if data.length === 0}
		<div class="empty-state flex items-center justify-center h-full">
			<p class="text-base-content/60 text-sm">No data to display</p>
		</div>
	{:else}
		<svg width={containerWidth} height={effectiveHeight}>
			{#each rectangles as rect}
				<g class="treemap-node">
					<rect
						x={rect.x}
						y={rect.y}
						width={rect.width}
						height={rect.height}
						fill={getColor(rect.colorIndex, rect.node)}
						stroke="#1e293b"
						stroke-width="2"
						rx="4"
						class="transition-all duration-200 hover:opacity-80 cursor-pointer"
					>
						<title>{rect.node.label}: {formatValue(rect.node.value)} ({((rect.node.value / totalValue) * 100).toFixed(1)}%)</title>
					</rect>

					{#if showLabels && shouldShowLabel(rect)}
						<text
							x={rect.x + rect.width / 2}
							y={rect.y + rect.height / 2 - (showValues && shouldShowValue(rect) ? 8 : 0)}
							text-anchor="middle"
							dominant-baseline="middle"
							fill={getTextColor(getColor(rect.colorIndex, rect.node))}
							class="text-sm font-semibold pointer-events-none"
						>
							{rect.node.label.length > 12 ? rect.node.label.slice(0, 12) + '..' : rect.node.label}
						</text>
					{/if}

					{#if showValues && shouldShowValue(rect)}
						<text
							x={rect.x + rect.width / 2}
							y={rect.y + rect.height / 2 + 12}
							text-anchor="middle"
							dominant-baseline="middle"
							fill={getTextColor(getColor(rect.colorIndex, rect.node))}
							class="text-xs pointer-events-none opacity-80"
						>
							{formatValue(rect.node.value)}
						</text>
					{/if}
				</g>
			{/each}
		</svg>
	{/if}
</div>
