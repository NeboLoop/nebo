<script lang="ts">
	import { onMount } from 'svelte';
	import ChartTooltip from './ChartTooltip.svelte';

	interface ChartJsData {
		labels?: string[];
		datasets?: Array<{
			label?: string;
			data: number[];
			backgroundColor?: string | string[];
			borderColor?: string;
		}>;
	}

	interface Props {
		data: ChartJsData;
		height?: number;
		showLegend?: boolean;
		showGrid?: boolean;
		orientation?: 'vertical' | 'horizontal';
		mode?: 'grouped' | 'stacked' | 'stacked100' | 'diverging';
		onBarClick?: (data: { label: string; value: number; datasetLabel: string; datasetIndex: number; labelIndex: number }) => void;
	}

	let {
		data,
		height,
		showLegend = true,
		showGrid = true,
		orientation = 'vertical',
		mode = 'grouped',
		onBarClick
	}: Props = $props();

	let containerRef = $state<HTMLDivElement | undefined>(undefined);
	let containerWidth = $state(400);
	let containerHeight = $state(300);

	// Tooltip state
	let tooltipVisible = $state(false);
	let tooltipX = $state(0);
	let tooltipY = $state(0);
	let tooltipData = $state<{
		label: string;
		value: string | number;
		percentage?: string;
		color?: string;
		secondaryLabel?: string;
		secondaryValue?: string;
	} | null>(null);

	// Hover state for highlighting
	let hoveredBar = $state<{ labelIndex: number; datasetIndex: number } | null>(null);

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
	const isHorizontal = $derived(orientation === 'horizontal');

	const padding = $derived({
		top: 20,
		right: 20,
		bottom: isHorizontal ? 40 : 40,
		left: isHorizontal ? 100 : 50
	});

	const viewBoxWidth = $derived(containerWidth);
	const viewBoxHeight = $derived(effectiveHeight);
	const chartWidth = $derived(viewBoxWidth - padding.left - padding.right);
	const chartHeight = $derived(viewBoxHeight - padding.top - padding.bottom);

	const defaultColors = [
		'rgba(99, 102, 241, 0.8)',
		'rgba(168, 85, 247, 0.8)',
		'rgba(236, 72, 153, 0.8)',
		'rgba(251, 146, 60, 0.8)',
		'rgba(34, 197, 94, 0.8)',
		'rgba(14, 165, 233, 0.8)',
	];

	const labels = $derived(data?.labels || []);
	const datasets = $derived(data?.datasets || []);

	// Calculate total for percentages
	const total = $derived.by(() => {
		let sum = 0;
		for (const dataset of datasets) {
			for (const value of dataset.data || []) {
				sum += Math.abs(value || 0);
			}
		}
		return sum || 1;
	});

	// Calculate stacked data
	const stackedData = $derived.by(() => {
		if (mode !== 'stacked' && mode !== 'stacked100' && mode !== 'diverging') return null;

		const result: { positive: number[][]; negative: number[][] } = { positive: [], negative: [] };

		for (let i = 0; i < labels.length; i++) {
			let positiveCumulative = 0;
			let negativeCumulative = 0;
			const positiveStack: number[] = [];
			const negativeStack: number[] = [];

			let total = 0;
			if (mode === 'stacked100') {
				for (const dataset of datasets) {
					total += Math.abs(dataset.data[i] || 0);
				}
			}

			for (const dataset of datasets) {
				let value = dataset.data[i] || 0;

				if (mode === 'stacked100' && total > 0) {
					value = (Math.abs(value) / total) * 100 * Math.sign(value || 1);
				}

				if (mode === 'diverging' || value >= 0) {
					positiveCumulative += value >= 0 ? value : 0;
					positiveStack.push(positiveCumulative);
					negativeCumulative += value < 0 ? value : 0;
					negativeStack.push(negativeCumulative);
				} else {
					positiveStack.push(positiveCumulative);
					negativeCumulative += value;
					negativeStack.push(negativeCumulative);
				}
			}

			result.positive.push(positiveStack);
			result.negative.push(negativeStack);
		}

		return result;
	});

	// Calculate max value
	const maxValue = $derived.by(() => {
		if (mode === 'stacked100') return 100;

		if ((mode === 'stacked' || mode === 'diverging') && stackedData) {
			let max = 0;
			for (const stack of stackedData.positive) {
				const lastValue = stack[stack.length - 1] || 0;
				if (lastValue > max) max = lastValue;
			}
			return max || 1;
		}

		let max = 0;
		for (const dataset of datasets) {
			for (const value of dataset.data || []) {
				if (Math.abs(value) > max) max = Math.abs(value);
			}
		}
		return max || 1;
	});

	const minValue = $derived.by(() => {
		if (mode === 'stacked100') return 0;

		if ((mode === 'stacked' || mode === 'diverging') && stackedData) {
			let min = 0;
			for (const stack of stackedData.negative) {
				const lastValue = stack[stack.length - 1] || 0;
				if (lastValue < min) min = lastValue;
			}
			return min;
		}

		let min = 0;
		for (const dataset of datasets) {
			for (const value of dataset.data || []) {
				if (value < min) min = value;
			}
		}
		return min;
	});

	const barGroupSize = $derived.by(() => {
		if (labels.length === 0) return 0;
		return isHorizontal ? chartHeight / labels.length : chartWidth / labels.length;
	});

	const barWidth = $derived.by(() => {
		if (mode === 'stacked' || mode === 'stacked100' || mode === 'diverging') {
			return barGroupSize * 0.7;
		}
		const numDatasets = datasets.length || 1;
		const groupPadding = barGroupSize * 0.2;
		return (barGroupSize - groupPadding) / numDatasets;
	});

	function valueScale(value: number): number {
		const range = maxValue - minValue || 1;
		if (isHorizontal) {
			return ((value - minValue) / range) * chartWidth;
		}
		return chartHeight - ((value - minValue) / range) * chartHeight;
	}

	const valueTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 4;
		const range = maxValue - minValue;
		for (let i = 0; i <= tickCount; i++) {
			const value = minValue + (range / tickCount) * i;
			const pos = isHorizontal
				? (chartWidth / tickCount) * i
				: chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, pos });
		}
		return ticks;
	});

	function formatValue(value: number): string {
		if (mode === 'stacked100') return `${value.toFixed(0)}%`;
		if (Math.abs(value) >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (Math.abs(value) >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toFixed(0);
	}

	function getBarColor(datasetIndex: number, pointIndex: number, dataset: any): string {
		if (Array.isArray(dataset.backgroundColor)) {
			return dataset.backgroundColor[pointIndex] || defaultColors[datasetIndex % defaultColors.length];
		}
		return dataset.backgroundColor || defaultColors[datasetIndex % defaultColors.length];
	}

	// Get bar position and size for each bar
	function getBarGeometry(labelIndex: number, datasetIndex: number) {
		const groupStart = labelIndex * barGroupSize + barGroupSize * 0.1;
		let value = datasets[datasetIndex]?.data[labelIndex] || 0;

		if (mode === 'stacked' || mode === 'stacked100') {
			const prevValue = datasetIndex > 0 ? stackedData!.positive[labelIndex][datasetIndex - 1] : 0;
			const currentValue = stackedData!.positive[labelIndex][datasetIndex];

			if (mode === 'stacked100') {
				let total = 0;
				for (const dataset of datasets) {
					total += Math.abs(dataset.data[labelIndex] || 0);
				}
				if (total > 0) {
					value = (Math.abs(value) / total) * 100;
				}
			}

			if (isHorizontal) {
				return {
					x: valueScale(prevValue),
					y: groupStart,
					width: valueScale(currentValue) - valueScale(prevValue),
					height: barWidth
				};
			} else {
				return {
					x: groupStart + (barGroupSize * 0.8 - barWidth) / 2,
					y: valueScale(currentValue),
					width: barWidth,
					height: valueScale(prevValue) - valueScale(currentValue)
				};
			}
		} else if (mode === 'diverging') {
			const zeroPos = valueScale(0);

			if (isHorizontal) {
				const barLength = Math.abs(valueScale(value) - zeroPos);
				return {
					x: value >= 0 ? zeroPos : zeroPos - barLength,
					y: groupStart,
					width: barLength,
					height: barWidth
				};
			} else {
				const barHeight = Math.abs(valueScale(value) - zeroPos);
				return {
					x: groupStart + (barGroupSize * 0.8 - barWidth) / 2,
					y: value >= 0 ? valueScale(value) : zeroPos,
					width: barWidth,
					height: barHeight
				};
			}
		} else {
			// Grouped mode
			const barStart = groupStart + datasetIndex * barWidth;

			if (isHorizontal) {
				return {
					x: valueScale(minValue),
					y: barStart,
					width: valueScale(value) - valueScale(minValue),
					height: barWidth * 0.9
				};
			} else {
				const barHeight = valueScale(minValue) - valueScale(value);
				return {
					x: barStart,
					y: valueScale(value),
					width: barWidth * 0.9,
					height: barHeight
				};
			}
		}
	}

	function handleBarMouseEnter(event: MouseEvent, labelIndex: number, datasetIndex: number, dataset: any) {
		const value = dataset.data[labelIndex] || 0;
		const label = labels[labelIndex] || '';
		const percentage = ((Math.abs(value) / total) * 100).toFixed(1) + '%';

		hoveredBar = { labelIndex, datasetIndex };

		if (!containerRef) return;
		const rect = containerRef.getBoundingClientRect();
		tooltipX = event.clientX - rect.left;
		tooltipY = event.clientY - rect.top;

		tooltipData = {
			label: dataset.label || label,
			value: value,
			percentage,
			color: getBarColor(datasetIndex, labelIndex, dataset),
			secondaryLabel: datasets.length > 1 ? 'Category' : undefined,
			secondaryValue: datasets.length > 1 ? label : undefined
		};
		tooltipVisible = true;
	}

	function handleBarMouseMove(event: MouseEvent) {
		if (tooltipVisible && containerRef) {
			const rect = containerRef.getBoundingClientRect();
			tooltipX = event.clientX - rect.left;
			tooltipY = event.clientY - rect.top;
		}
	}

	function handleBarMouseLeave() {
		hoveredBar = null;
		tooltipVisible = false;
		tooltipData = null;
	}

	function handleBarClick(labelIndex: number, datasetIndex: number, dataset: any) {
		if (onBarClick) {
			onBarClick({
				label: labels[labelIndex] || '',
				value: dataset.data[labelIndex] || 0,
				datasetLabel: dataset.label || '',
				datasetIndex,
				labelIndex
			});
		}
	}

	function isBarDimmed(labelIndex: number, datasetIndex: number): boolean {
		if (!hoveredBar) return false;
		return hoveredBar.labelIndex !== labelIndex || hoveredBar.datasetIndex !== datasetIndex;
	}

	function isBarHighlighted(labelIndex: number, datasetIndex: number): boolean {
		if (!hoveredBar) return false;
		return hoveredBar.labelIndex === labelIndex && hoveredBar.datasetIndex === datasetIndex;
	}
</script>

<div bind:this={containerRef} class="flex flex-col w-full h-full relative">
	{#if labels.length === 0 || datasets.length === 0}
		<div class="flex-1 flex items-center justify-center">
			<p class="text-base-500 text-sm">No data to display</p>
		</div>
	{:else}
		<svg viewBox="0 0 {viewBoxWidth} {viewBoxHeight}" preserveAspectRatio="none" class="flex-1 w-full">
			<!-- Grid lines -->
			{#if showGrid}
				<g transform="translate({padding.left}, {padding.top})">
					{#each valueTicks as tick}
						{#if isHorizontal}
							<line
								x1={tick.pos}
								y1="0"
								x2={tick.pos}
								y2={chartHeight}
								stroke="rgba(51, 65, 85, 0.3)"
								stroke-width="1"
								stroke-dasharray="4,4"
							/>
						{:else}
							<line
								x1="0"
								y1={tick.pos}
								x2={chartWidth}
								y2={tick.pos}
								stroke="rgba(51, 65, 85, 0.3)"
								stroke-width="1"
								stroke-dasharray="4,4"
							/>
						{/if}
					{/each}
				</g>
			{/if}

			<!-- Zero line for diverging -->
			{#if mode === 'diverging' && minValue < 0}
				<g transform="translate({padding.left}, {padding.top})">
					{#if isHorizontal}
						<line
							x1={valueScale(0)}
							y1="0"
							x2={valueScale(0)}
							y2={chartHeight}
							stroke="#94a3b8"
							stroke-width="2"
						/>
					{:else}
						<line
							x1="0"
							y1={valueScale(0)}
							x2={chartWidth}
							y2={valueScale(0)}
							stroke="#94a3b8"
							stroke-width="2"
						/>
					{/if}
				</g>
			{/if}

			<!-- Bars -->
			<g transform="translate({padding.left}, {padding.top})">
				{#if mode === 'stacked' || mode === 'stacked100'}
					<!-- Stacked: render datasets in order -->
					{#each labels as label, labelIndex}
						{#each datasets as dataset, datasetIndex}
							{@const geom = getBarGeometry(labelIndex, datasetIndex)}
							<rect
								x={geom.x}
								y={geom.y}
								width={Math.max(geom.width, 0)}
								height={Math.max(geom.height, 0)}
								fill={getBarColor(datasetIndex, labelIndex, dataset)}
								rx="2"
								class="transition-all duration-150 cursor-pointer outline-none focus:outline-none {isBarHighlighted(labelIndex, datasetIndex) ? 'brightness-110' : ''} {isBarDimmed(labelIndex, datasetIndex) ? 'opacity-40' : ''}"
								onmouseenter={(e) => handleBarMouseEnter(e, labelIndex, datasetIndex, dataset)}
								onmousemove={handleBarMouseMove}
								onmouseleave={handleBarMouseLeave}
								onclick={() => handleBarClick(labelIndex, datasetIndex, dataset)}
								role="button"
								tabindex="0"
								onkeydown={(e) => e.key === 'Enter' && handleBarClick(labelIndex, datasetIndex, dataset)}
							/>
						{/each}
					{/each}
				{:else}
					<!-- Grouped or Diverging -->
					{#each labels as label, labelIndex}
						{#each datasets as dataset, datasetIndex}
							{@const geom = getBarGeometry(labelIndex, datasetIndex)}
							<rect
								x={geom.x}
								y={geom.y}
								width={Math.max(geom.width, 0)}
								height={Math.max(geom.height, 0)}
								fill={getBarColor(datasetIndex, labelIndex, dataset)}
								rx="2"
								class="transition-all duration-150 cursor-pointer outline-none focus:outline-none {isBarHighlighted(labelIndex, datasetIndex) ? 'brightness-110' : ''} {isBarDimmed(labelIndex, datasetIndex) ? 'opacity-40' : ''}"
								onmouseenter={(e) => handleBarMouseEnter(e, labelIndex, datasetIndex, dataset)}
								onmousemove={handleBarMouseMove}
								onmouseleave={handleBarMouseLeave}
								onclick={() => handleBarClick(labelIndex, datasetIndex, dataset)}
								role="button"
								tabindex="0"
								onkeydown={(e) => e.key === 'Enter' && handleBarClick(labelIndex, datasetIndex, dataset)}
							/>
						{/each}
					{/each}
				{/if}
			</g>

			<!-- Value axis -->
			<g transform="translate({padding.left}, {padding.top})">
				{#if isHorizontal}
					<!-- X-axis for horizontal bars (value axis) -->
					<line x1="0" y1={chartHeight} x2={chartWidth} y2={chartHeight} stroke="#475569" stroke-width="1"/>
					{#each valueTicks as tick}
						<text
							x={tick.pos}
							y={chartHeight + 20}
							text-anchor="middle"
							class="text-xs fill-current text-text-muted"
							font-size="10"
						>
							{formatValue(tick.value)}
						</text>
					{/each}
				{:else}
					<!-- Y-axis for vertical bars (value axis) -->
					<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="#475569" stroke-width="1"/>
					{#each valueTicks as tick}
						<text
							x="-8"
							y={tick.pos}
							text-anchor="end"
							dominant-baseline="middle"
							class="text-xs fill-current text-text-muted"
							font-size="10"
						>
							{formatValue(tick.value)}
						</text>
					{/each}
				{/if}
			</g>

			<!-- Category axis -->
			<g transform="translate({padding.left}, {padding.top})">
				{#if isHorizontal}
					<!-- Y-axis for horizontal bars (category axis) -->
					<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="#475569" stroke-width="1"/>
					{#each labels as label, i}
						<text
							x="-8"
							y={i * barGroupSize + barGroupSize / 2}
							text-anchor="end"
							dominant-baseline="middle"
							class="text-xs fill-current text-text-muted"
							font-size="10"
						>
							{label.length > 12 ? label.slice(0, 12) + '..' : label}
						</text>
					{/each}
				{:else}
					<!-- X-axis for vertical bars (category axis) -->
					<line x1="0" y1={chartHeight} x2={chartWidth} y2={chartHeight} stroke="#475569" stroke-width="1"/>
					{#each labels as label, i}
						<text
							x={i * barGroupSize + barGroupSize / 2}
							y={chartHeight + 20}
							text-anchor="middle"
							class="text-xs fill-current text-text-muted"
							font-size="10"
						>
							{label.length > 10 ? label.slice(0, 10) + '..' : label}
						</text>
					{/each}
				{/if}
			</g>
		</svg>

		<!-- Legend -->
		{#if showLegend && datasets.length > 1}
			<div class="flex flex-wrap justify-center gap-4 pt-3">
				{#each datasets as dataset, i}
					<div class="flex items-center gap-1.5">
						<span class="w-3 h-3 rounded-full" style="background-color: {getBarColor(i, 0, dataset)}"></span>
						<span class="text-xs text-text-muted">{dataset.label || `Series ${i + 1}`}</span>
					</div>
				{/each}
			</div>
		{/if}

		<!-- Tooltip -->
		<ChartTooltip
			visible={tooltipVisible}
			x={tooltipX}
			y={tooltipY}
			data={tooltipData}
			containerRef={containerRef}
		/>
	{/if}
</div>
