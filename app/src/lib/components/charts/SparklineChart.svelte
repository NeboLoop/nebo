<script lang="ts">
	interface Props {
		data: number[] | { data: number[] };
		width?: number;
		height?: number;
		color?: string;
		fill?: boolean;
		showEndDot?: boolean;
		showMinMax?: boolean;
		type?: 'line' | 'bar' | 'area';
	}

	let {
		data: rawData = [],
		width = 100,
		height = 30,
		color = 'rgba(99, 102, 241, 1)',
		fill = false,
		showEndDot = true,
		showMinMax = false,
		type = 'line'
	}: Props = $props();

	// Normalize data - handle both direct array and { data: [...] } structure
	const data = $derived<number[]>(
		Array.isArray(rawData) ? rawData : (rawData?.data && Array.isArray(rawData.data) ? rawData.data : [])
	);

	const padding = 2;
	const chartWidth = $derived(width - padding * 2);
	const chartHeight = $derived(height - padding * 2);

	const dataRange = $derived.by(() => {
		if (data.length === 0) return { min: 0, max: 1 };
		const min = Math.min(...data);
		const max = Math.max(...data);
		const range = max - min || 1;
		return { min: min - range * 0.1, max: max + range * 0.1 };
	});

	const minMaxIndices = $derived.by(() => {
		if (data.length === 0) return { min: -1, max: -1 };
		let minIdx = 0;
		let maxIdx = 0;
		for (let i = 1; i < data.length; i++) {
			if (data[i] < data[minIdx]) minIdx = i;
			if (data[i] > data[maxIdx]) maxIdx = i;
		}
		return { min: minIdx, max: maxIdx };
	});

	function xScale(index: number): number {
		if (data.length <= 1) return chartWidth / 2;
		return (index / (data.length - 1)) * chartWidth;
	}

	function yScale(value: number): number {
		return chartHeight - ((value - dataRange.min) / (dataRange.max - dataRange.min)) * chartHeight;
	}

	const linePath = $derived.by(() => {
		if (data.length === 0) return '';
		return data.map((value, i) => {
			const x = xScale(i);
			const y = yScale(value);
			return `${i === 0 ? 'M' : 'L'}${x},${y}`;
		}).join(' ');
	});

	const areaPath = $derived.by(() => {
		if (data.length === 0) return '';
		const line = linePath;
		const lastX = xScale(data.length - 1);
		const firstX = xScale(0);
		return `${line} L${lastX},${chartHeight} L${firstX},${chartHeight} Z`;
	});

	const lastPoint = $derived.by(() => {
		if (data.length === 0) return null;
		return {
			x: xScale(data.length - 1),
			y: yScale(data[data.length - 1])
		};
	});

	// Determine trend color
	const trendColor = $derived.by(() => {
		if (data.length < 2) return color;
		const firstValue = data[0];
		const lastValue = data[data.length - 1];
		if (lastValue > firstValue) return 'rgba(34, 197, 94, 1)'; // Green
		if (lastValue < firstValue) return 'rgba(239, 68, 68, 1)'; // Red
		return color;
	});

	const fillColor = $derived(trendColor.replace('1)', '0.2)'));

	const barWidth = $derived.by(() => {
		if (data.length === 0) return 0;
		return Math.max((chartWidth / data.length) * 0.8, 2);
	});
</script>

<svg {width} {height} class="sparkline-chart">
	<g transform="translate({padding}, {padding})">
		{#if type === 'line' || type === 'area'}
			<!-- Area fill -->
			{#if fill || type === 'area'}
				<path
					d={areaPath}
					fill={fillColor}
				/>
			{/if}

			<!-- Line -->
			<path
				d={linePath}
				fill="none"
				stroke={trendColor}
				stroke-width="1.5"
				stroke-linecap="round"
				stroke-linejoin="round"
			/>

			<!-- End dot -->
			{#if showEndDot && lastPoint}
				<circle
					cx={lastPoint.x}
					cy={lastPoint.y}
					r="2.5"
					fill={trendColor}
				/>
			{/if}

			<!-- Min/Max indicators -->
			{#if showMinMax && data.length > 2}
				<circle
					cx={xScale(minMaxIndices.min)}
					cy={yScale(data[minMaxIndices.min])}
					r="2"
					fill="rgba(239, 68, 68, 1)"
				/>
				<circle
					cx={xScale(minMaxIndices.max)}
					cy={yScale(data[minMaxIndices.max])}
					r="2"
					fill="rgba(34, 197, 94, 1)"
				/>
			{/if}
		{:else if type === 'bar'}
			<!-- Bar chart -->
			{#each data as value, i}
				{@const barHeight = ((value - dataRange.min) / (dataRange.max - dataRange.min)) * chartHeight}
				<rect
					x={xScale(i) - barWidth / 2}
					y={chartHeight - barHeight}
					width={barWidth}
					height={barHeight}
					fill={trendColor}
					rx="1"
				/>
			{/each}
		{/if}
	</g>
</svg>
