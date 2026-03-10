<script lang="ts">
	import { onMount } from 'svelte';

	interface CandlestickData {
		date: Date | string;
		open: number;
		high: number;
		low: number;
		close: number;
		volume?: number;
	}

	interface Props {
		data: CandlestickData[];
		height?: number;
		showGrid?: boolean;
		showVolume?: boolean;
		bullColor?: string;
		bearColor?: string;
	}

	let {
		data = [],
		height,
		showGrid = true,
		showVolume = false,
		bullColor = 'rgba(34, 197, 94, 0.85)',
		bearColor = 'rgba(239, 68, 68, 0.85)'
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
	const padding = { top: 20, right: 60, bottom: 50, left: 60 };
	const volumeHeight = $derived(showVolume ? 60 : 0);
	const chartWidth = $derived(containerWidth - padding.left - padding.right);
	const chartHeight = $derived(effectiveHeight - padding.top - padding.bottom - volumeHeight);

	// Parse data and calculate ranges
	const parsedData = $derived(data.map(d => ({
		...d,
		date: d.date instanceof Date ? d.date : new Date(d.date)
	})));

	const priceRange = $derived.by(() => {
		if (parsedData.length === 0) return { min: 0, max: 100 };
		let min = Infinity;
		let max = -Infinity;
		for (const d of parsedData) {
			min = Math.min(min, d.low);
			max = Math.max(max, d.high);
		}
		const padding = (max - min) * 0.05;
		return { min: min - padding, max: max + padding };
	});

	const volumeMax = $derived.by(() => {
		if (!showVolume) return 1;
		let max = 0;
		for (const d of parsedData) {
			if (d.volume && d.volume > max) max = d.volume;
		}
		return max || 1;
	});

	const candleWidth = $derived.by(() => {
		if (parsedData.length === 0) return 0;
		return Math.max((chartWidth / parsedData.length) * 0.7, 3);
	});

	function xScale(index: number): number {
		return ((index + 0.5) / parsedData.length) * chartWidth;
	}

	function yScale(value: number): number {
		return chartHeight - ((value - priceRange.min) / (priceRange.max - priceRange.min)) * chartHeight;
	}

	function volumeScale(value: number): number {
		return volumeHeight - (value / volumeMax) * volumeHeight * 0.8;
	}

	const yTicks = $derived.by(() => {
		const ticks = [];
		const tickCount = 5;
		const range = priceRange.max - priceRange.min;
		for (let i = 0; i <= tickCount; i++) {
			const value = priceRange.min + (range / tickCount) * i;
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y });
		}
		return ticks;
	});

	function formatPrice(value: number): string {
		if (value >= 1000) return `$${(value / 1000).toFixed(1)}K`;
		return `$${value.toFixed(2)}`;
	}

	function formatDate(date: Date): string {
		return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
	}

	function isBullish(d: CandlestickData): boolean {
		return d.close >= d.open;
	}
</script>

<div bind:this={containerRef} class="w-full h-full">
	{#if data.length === 0}
		<div class="flex items-center justify-center h-full">
			<p class="text-base-500 text-sm">No data to display</p>
		</div>
	{:else}
		<svg width={containerWidth} height={effectiveHeight}>
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

			<!-- Candlesticks -->
			<g transform="translate({padding.left}, {padding.top})">
				{#each parsedData as candle, i}
					{@const x = xScale(i)}
					{@const bullish = isBullish(candle)}
					{@const color = bullish ? bullColor : bearColor}
					{@const bodyTop = yScale(Math.max(candle.open, candle.close))}
					{@const bodyBottom = yScale(Math.min(candle.open, candle.close))}
					{@const bodyHeight = Math.max(bodyBottom - bodyTop, 1)}

					<!-- High-Low wick -->
					<line
						x1={x}
						y1={yScale(candle.high)}
						x2={x}
						y2={yScale(candle.low)}
						stroke={color}
						stroke-width="1"
					/>

					<!-- Body -->
					<rect
						x={x - candleWidth / 2}
						y={bodyTop}
						width={candleWidth}
						height={bodyHeight}
						fill={bullish ? color : color}
						stroke={color}
						stroke-width="1"
						class="transition-all duration-200 hover:opacity-80 cursor-pointer"
					>
						<title>
							Date: {formatDate(candle.date)}
							Open: {formatPrice(candle.open)}
							High: {formatPrice(candle.high)}
							Low: {formatPrice(candle.low)}
							Close: {formatPrice(candle.close)}
							{candle.volume ? `Volume: ${candle.volume.toLocaleString()}` : ''}
						</title>
					</rect>
				{/each}
			</g>

			<!-- Volume bars -->
			{#if showVolume}
				<g transform="translate({padding.left}, {padding.top + chartHeight + 10})">
					{#each parsedData as candle, i}
						{#if candle.volume}
							{@const x = xScale(i)}
							{@const bullish = isBullish(candle)}
							{@const color = bullish ? bullColor : bearColor}
							{@const barHeight = volumeHeight - volumeScale(candle.volume)}
							<rect
								x={x - candleWidth / 2}
								y={volumeHeight - barHeight}
								width={candleWidth}
								height={barHeight}
								fill={color}
								opacity="0.5"
							/>
						{/if}
					{/each}
				</g>
			{/if}

			<!-- Y-axis -->
			<g transform="translate({padding.left}, {padding.top})">
				<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="#475569" stroke-width="1"/>
				{#each yTicks as tick}
					<text
						x="-8"
						y={tick.y}
						text-anchor="end"
						dominant-baseline="middle"
						class="text-xs fill-base-500"
						font-size="10"
					>
						{formatPrice(tick.value)}
					</text>
				{/each}
			</g>

			<!-- X-axis -->
			<g transform="translate({padding.left}, {padding.top + chartHeight + (showVolume ? volumeHeight + 10 : 0)})">
				<line x1="0" y1="0" x2={chartWidth} y2="0" stroke="#475569" stroke-width="1"/>
				{#each parsedData as candle, i}
					{#if i % Math.ceil(parsedData.length / 6) === 0}
						<text
							x={xScale(i)}
							y="20"
							text-anchor="middle"
							class="text-xs fill-base-500"
							font-size="10"
						>
							{formatDate(candle.date)}
						</text>
					{/if}
				{/each}
			</g>
		</svg>

		<!-- Legend -->
		<div class="flex flex-wrap justify-center gap-4 mt-2">
			<div class="flex items-center gap-1.5">
				<span class="w-3 h-3 rounded-full" style="background-color: {bullColor}"></span>
				<span class="text-xs text-text-muted">Bullish</span>
			</div>
			<div class="flex items-center gap-1.5">
				<span class="w-3 h-3 rounded-full" style="background-color: {bearColor}"></span>
				<span class="text-xs text-text-muted">Bearish</span>
			</div>
		</div>
	{/if}
</div>
