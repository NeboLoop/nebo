<script lang="ts">
	import { onMount } from 'svelte';

	interface WaterfallItem {
		label: string;
		value: number;
		type?: 'increase' | 'decrease' | 'total';
	}

	interface Props {
		data: WaterfallItem[];
		height?: number;
		showGrid?: boolean;
		showValues?: boolean;
		showConnectors?: boolean;
	}

	let {
		data = [],
		height,
		showGrid = true,
		showValues = true,
		showConnectors = true
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
	const padding = { top: 20, right: 20, bottom: 60, left: 60 };
	const viewBoxWidth = $derived(containerWidth);
	const viewBoxHeight = $derived(effectiveHeight);
	const chartWidth = $derived(viewBoxWidth - padding.left - padding.right);
	const chartHeight = $derived(viewBoxHeight - padding.top - padding.bottom);

	// Calculate cumulative values and determine bar positions
	const processedData = $derived.by(() => {
		const result: Array<{
			label: string;
			value: number;
			type: 'increase' | 'decrease' | 'total';
			startValue: number;
			endValue: number;
		}> = [];

		let cumulative = 0;

		for (const item of data) {
			const type = item.type || (item.value >= 0 ? 'increase' : 'decrease');

			if (type === 'total') {
				result.push({
					label: item.label,
					value: item.value,
					type: 'total',
					startValue: 0,
					endValue: item.value
				});
				cumulative = item.value;
			} else {
				const startValue = cumulative;
				cumulative += item.value;
				result.push({
					label: item.label,
					value: item.value,
					type,
					startValue,
					endValue: cumulative
				});
			}
		}

		return result;
	});

	// Calculate value range
	const valueRange = $derived.by(() => {
		if (processedData.length === 0) return { min: 0, max: 100 };

		let min = 0;
		let max = 0;

		for (const item of processedData) {
			min = Math.min(min, item.startValue, item.endValue);
			max = Math.max(max, item.startValue, item.endValue);
		}

		const padding = (max - min) * 0.1;
		return { min: min - padding, max: max + padding };
	});

	const barWidth = $derived.by(() => {
		if (processedData.length === 0) return 0;
		return (chartWidth / processedData.length) * 0.6;
	});

	function xScale(index: number): number {
		return (index + 0.5) * (chartWidth / processedData.length);
	}

	function yScale(value: number): number {
		const range = valueRange.max - valueRange.min;
		return chartHeight - ((value - valueRange.min) / range) * chartHeight;
	}

	const yTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 5;
		const range = valueRange.max - valueRange.min;
		for (let i = 0; i <= tickCount; i++) {
			const value = valueRange.min + (range / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	function getBarColor(type: 'increase' | 'decrease' | 'total'): string {
		switch (type) {
			case 'increase':
				return 'rgba(34, 197, 94, 0.85)'; // Green
			case 'decrease':
				return 'rgba(239, 68, 68, 0.85)'; // Red
			case 'total':
				return 'rgba(99, 102, 241, 0.85)'; // Indigo
		}
	}

	function formatValue(value: number): string {
		const absValue = Math.abs(value);
		if (absValue >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (absValue >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toLocaleString();
	}
</script>

<div bind:this={containerRef} class="waterfall-chart-container flex flex-col w-full h-full">
	{#if data.length === 0}
		<div class="empty-state flex-1 flex items-center justify-center">
			<p class="text-base-content/60 text-sm">No data to display</p>
		</div>
	{:else}
		<svg viewBox="0 0 {viewBoxWidth} {viewBoxHeight}" preserveAspectRatio="xMidYMid meet" class="flex-1 w-full">
			<!-- Grid lines -->
			{#if showGrid}
				<g transform="translate({padding.left}, {padding.top})">
					{#each yTicks as tick}
						<line
							x1="0"
							y1={tick.y}
							x2={chartWidth}
							y2={tick.y}
							stroke="rgba(51, 65, 85, 0.3)"
							stroke-width="1"
							stroke-dasharray="4,4"
						/>
					{/each}
				</g>
			{/if}

			<!-- Zero line if applicable -->
			{#if valueRange.min < 0 && valueRange.max > 0}
				<g transform="translate({padding.left}, {padding.top})">
					<line
						x1="0"
						y1={yScale(0)}
						x2={chartWidth}
						y2={yScale(0)}
						stroke="#475569"
						stroke-width="2"
					/>
				</g>
			{/if}

			<!-- Connectors -->
			{#if showConnectors}
				<g transform="translate({padding.left}, {padding.top})">
					{#each processedData as item, i}
						{#if i < processedData.length - 1}
							{@const nextItem = processedData[i + 1]}
							{@const connectorY = item.type === 'total' ? yScale(item.endValue) : yScale(item.endValue)}
							<line
								x1={xScale(i) + barWidth / 2}
								y1={connectorY}
								x2={xScale(i + 1) - barWidth / 2}
								y2={connectorY}
								stroke="rgba(148, 163, 184, 0.5)"
								stroke-width="1"
								stroke-dasharray="3,3"
							/>
						{/if}
					{/each}
				</g>
			{/if}

			<!-- Bars -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each processedData as item, i}
					{@const barTop = Math.min(yScale(item.startValue), yScale(item.endValue))}
					{@const barBottom = Math.max(yScale(item.startValue), yScale(item.endValue))}
					{@const barHeight = barBottom - barTop}

					<rect
						x={xScale(i) - barWidth / 2}
						y={barTop}
						width={barWidth}
						height={Math.max(barHeight, 2)}
						fill={getBarColor(item.type)}
						stroke="#1e293b"
						stroke-width="1"
						rx="2"
						class="transition-all duration-200 hover:opacity-80 cursor-pointer"
					>
						<title>{item.label}: {formatValue(item.value)}</title>
					</rect>

					<!-- Value label -->
					{#if showValues}
						<text
							x={xScale(i)}
							y={barTop - 8}
							text-anchor="middle"
							class="text-xs fill-base-content/70 font-medium"
						>
							{item.value >= 0 ? '+' : ''}{formatValue(item.value)}
						</text>
					{/if}
				{/each}
			</g>

			<!-- Y-axis -->
			<g transform="translate({padding.left}, {padding.top})">
				<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="#475569" stroke-width="1"/>
				{#each yTicks as tick}
					<text
						x="-8"
						y={tick.y}
						text-anchor="end"
						dominant-baseline="middle"
						class="text-xs fill-base-content/60"
						font-size="10"
					>
						{formatValue(tick.value)}
					</text>
				{/each}
			</g>

			<!-- X-axis -->
			<g transform="translate({padding.left}, {padding.top + chartHeight})">
				<line x1="0" y1="0" x2={chartWidth} y2="0" stroke="#475569" stroke-width="1"/>
				{#each processedData as item, i}
					<text
						x={xScale(i)}
						y="20"
						text-anchor="middle"
						class="text-xs fill-base-content/60"
						font-size="10"
						transform="rotate(-30, {xScale(i)}, 20)"
					>
						{item.label.length > 10 ? item.label.slice(0, 10) + '..' : item.label}
					</text>
				{/each}
			</g>
		</svg>

		<!-- Legend -->
		<div class="legend">
			<div class="legend-item">
				<span class="legend-dot" style="background-color: rgba(34, 197, 94, 0.85)"></span>
				<span class="legend-label">Increase</span>
			</div>
			<div class="legend-item">
				<span class="legend-dot" style="background-color: rgba(239, 68, 68, 0.85)"></span>
				<span class="legend-label">Decrease</span>
			</div>
			<div class="legend-item">
				<span class="legend-dot" style="background-color: rgba(99, 102, 241, 0.85)"></span>
				<span class="legend-label">Total</span>
			</div>
		</div>
	{/if}
</div>
