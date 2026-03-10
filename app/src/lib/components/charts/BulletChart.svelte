<script lang="ts">
	import { onMount } from 'svelte';

	interface BulletData {
		value: number;
		target?: number;
		ranges?: number[];
		min?: number;
		max?: number;
		label?: string;
	}

	interface Props {
		data?: BulletData;
		value?: number;
		target?: number;
		ranges?: number[]; // [poor, satisfactory, good] thresholds as percentages of max
		min?: number;
		max?: number;
		label?: string;
		height?: number;
		orientation?: 'horizontal' | 'vertical';
	}

	let {
		data,
		value: propValue,
		target: propTarget,
		ranges: propRanges = [30, 70, 100],
		min: propMin = 0,
		max: propMax = 100,
		label: propLabel,
		height = 60,
		orientation = 'horizontal'
	}: Props = $props();

	// Extract values from data object or use direct props
	const value = $derived(data?.value ?? propValue ?? 0);
	const target = $derived(data?.target ?? propTarget);
	const ranges = $derived(data?.ranges ?? propRanges);
	const min = $derived(data?.min ?? propMin);
	const max = $derived(data?.max ?? propMax);
	const label = $derived(data?.label ?? propLabel);

	let containerRef: HTMLDivElement;
	let containerWidth = $state(300);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					containerWidth = Math.max(entry.contentRect.width, 150);
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	const isHorizontal = $derived(orientation === 'horizontal');
	const padding = { left: 100, right: 20, top: 10, bottom: 10 };

	const chartWidth = $derived(isHorizontal ? containerWidth - padding.left - padding.right : height - padding.top - padding.bottom);
	const chartHeight = $derived(isHorizontal ? height - padding.top - padding.bottom : containerWidth - padding.left - padding.right);
	const barHeight = $derived(chartHeight * 0.6);
	const markerHeight = $derived(chartHeight * 0.8);

	const rangeColors = [
		'rgba(51, 65, 85, 0.6)',    // Darkest - poor
		'rgba(71, 85, 105, 0.5)',   // Medium - satisfactory
		'rgba(100, 116, 139, 0.4)', // Lightest - good
	];

	function scale(val: number): number {
		return ((val - min) / (max - min)) * chartWidth;
	}

	const rangeWidths = $derived.by(() => {
		let prevValue = 0;
		return ranges.map((rangeValue, i) => {
			const scaledValue = (rangeValue / 100) * max;
			const width = scale(scaledValue) - scale(prevValue + min);
			const x = scale(prevValue + min);
			prevValue = scaledValue;
			return { x, width, color: rangeColors[i % rangeColors.length] };
		});
	});

	const valueWidth = $derived(scale(Math.min(value, max)));
	const targetX = $derived(target !== undefined ? scale(Math.min(target, max)) : null);

	function formatValue(val: number): string {
		if (Math.abs(val) >= 1000000) return `${(val / 1000000).toFixed(1)}M`;
		if (Math.abs(val) >= 1000) return `${(val / 1000).toFixed(1)}K`;
		return val.toFixed(0);
	}
</script>

<div bind:this={containerRef} class="bullet-chart-container w-full" style="height: {height}px;">
	{#if isHorizontal}
		<svg width={containerWidth} height={height} class="overflow-visible">
			<!-- Label -->
			{#if label}
				<text
					x={padding.left - 10}
					y={height / 2}
					text-anchor="end"
					dominant-baseline="middle"
					class="fill-base-content/70 text-sm font-medium"
				>
					{label}
				</text>
			{/if}

			<g transform="translate({padding.left}, {padding.top})">
				<!-- Range backgrounds -->
				{#each rangeWidths as range}
					<rect
						x={range.x}
						y={(chartHeight - barHeight) / 2}
						width={range.width}
						height={barHeight}
						fill={range.color}
						rx="2"
					/>
				{/each}

				<!-- Value bar -->
				<rect
					x="0"
					y={(chartHeight - barHeight * 0.4) / 2}
					width={valueWidth}
					height={barHeight * 0.4}
					fill="rgba(99, 102, 241, 0.9)"
					rx="2"
					class="transition-all duration-300"
				>
					<title>Value: {formatValue(value)}</title>
				</rect>

				<!-- Target marker -->
				{#if targetX !== null}
					<line
						x1={targetX}
						y1={(chartHeight - markerHeight) / 2}
						x2={targetX}
						y2={(chartHeight + markerHeight) / 2}
						stroke="#f8fafc"
						stroke-width="3"
					>
						<title>Target: {formatValue(target!)}</title>
					</line>
				{/if}

				<!-- Value label -->
				<text
					x={chartWidth + 10}
					y={chartHeight / 2}
					text-anchor="start"
					dominant-baseline="middle"
					class="fill-base-content/70 text-sm font-semibold"
				>
					{formatValue(value)}
				</text>
			</g>
		</svg>
	{:else}
		<!-- Vertical orientation -->
		<svg width={height} height={containerWidth} class="overflow-visible">
			<g transform="translate({padding.left}, {padding.top})">
				<!-- Range backgrounds (rotated) -->
				{#each rangeWidths as range}
					<rect
						x={(chartHeight - barHeight) / 2}
						y={chartWidth - range.x - range.width}
						width={barHeight}
						height={range.width}
						fill={range.color}
						rx="2"
					/>
				{/each}

				<!-- Value bar -->
				<rect
					x={(chartHeight - barHeight * 0.4) / 2}
					y={chartWidth - valueWidth}
					width={barHeight * 0.4}
					height={valueWidth}
					fill="rgba(99, 102, 241, 0.9)"
					rx="2"
					class="transition-all duration-300"
				/>

				<!-- Target marker -->
				{#if targetX !== null}
					<line
						x1={(chartHeight - markerHeight) / 2}
						y1={chartWidth - targetX}
						x2={(chartHeight + markerHeight) / 2}
						y2={chartWidth - targetX}
						stroke="#f8fafc"
						stroke-width="3"
					/>
				{/if}
			</g>

			<!-- Label -->
			{#if label}
				<text
					x={height / 2}
					y={containerWidth - 5}
					text-anchor="middle"
					class="fill-base-content/70 text-sm font-medium"
				>
					{label}
				</text>
			{/if}
		</svg>
	{/if}
</div>
