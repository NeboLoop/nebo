<script lang="ts">
	interface DataPoint {
		x: number;
		y: number;
		label?: string;
	}

	interface Props {
		data: DataPoint[];
		title?: string;
		xLabel?: string;
		yLabel?: string;
		width?: number;
		height?: number;
		color?: string;
		type?: 'line' | 'bar' | 'area';
		showGrid?: boolean;
		showDots?: boolean;
		formatX?: (value: number) => string;
		formatY?: (value: number) => string;
	}

	let {
		data,
		title,
		xLabel,
		yLabel,
		width = 600,
		height = 400,
		color = '#3B82F6',
		type = 'line',
		showGrid = true,
		showDots = true,
		formatX = (value: number) => value.toString(),
		formatY = (value: number) => value.toString()
	}: Props = $props();

	// Chart dimensions and padding
	const padding = { top: 40, right: 40, bottom: 60, left: 80 };
	const chartWidth = $derived(width - padding.left - padding.right);
	const chartHeight = $derived(height - padding.top - padding.bottom);

	// Calculate data bounds
	const xExtent = $derived.by(() => {
		if (data.length === 0) return [0, 1] as [number, number];
		const xValues = data.map(d => d.x);
		return [Math.min(...xValues), Math.max(...xValues)] as [number, number];
	});

	const yExtent = $derived.by(() => {
		if (data.length === 0) return [0, 1] as [number, number];
		const yValues = data.map(d => d.y);
		const min = Math.min(...yValues);
		const max = Math.max(...yValues);
		// Add some padding to y-axis
		const range = max - min;
		return [min - range * 0.1, max + range * 0.1] as [number, number];
	});

	// Scale functions
	function xScale(value: number): number {
		return ((value - xExtent[0]) / (xExtent[1] - xExtent[0])) * chartWidth;
	}

	function yScale(value: number): number {
		return chartHeight - ((value - yExtent[0]) / (yExtent[1] - yExtent[0])) * chartHeight;
	}

	// Generate path for line/area chart
	const linePath = $derived.by(() => {
		if (data.length === 0) return '';

		const points = data.map(d => `${xScale(d.x)},${yScale(d.y)}`);
		return `M${points.join('L')}`;
	});

	const areaPath = $derived.by(() => {
		if (data.length === 0 || type !== 'area') return '';

		const points = data.map(d => `${xScale(d.x)},${yScale(d.y)}`);
		const firstPoint = `${xScale(data[0].x)},${yScale(yExtent[0])}`;
		const lastPoint = `${xScale(data[data.length - 1].x)},${yScale(yExtent[0])}`;

		return `M${firstPoint}L${points.join('L')}L${lastPoint}Z`;
	});

	// Generate grid lines
	const gridLines = $derived.by(() => {
		const lines: { type: string; x1: number; y1: number; x2: number; y2: number }[] = [];
		const xTickCount = 5;
		const yTickCount = 5;

		// Vertical grid lines
		for (let i = 0; i <= xTickCount; i++) {
			const x = (chartWidth / xTickCount) * i;
			lines.push({
				type: 'vertical',
				x1: x,
				y1: 0,
				x2: x,
				y2: chartHeight
			});
		}

		// Horizontal grid lines
		for (let i = 0; i <= yTickCount; i++) {
			const y = (chartHeight / yTickCount) * i;
			lines.push({
				type: 'horizontal',
				x1: 0,
				y1: y,
				x2: chartWidth,
				y2: y
			});
		}

		return lines;
	});

	// Generate axis ticks
	const xAxisTicks = $derived.by(() => {
		const ticks: { value: number; x: number; label: string }[] = [];
		const tickCount = 5;
		for (let i = 0; i <= tickCount; i++) {
			const value = xExtent[0] + (xExtent[1] - xExtent[0]) * (i / tickCount);
			const x = (chartWidth / tickCount) * i;
			ticks.push({ value, x, label: formatX(value) });
		}
		return ticks;
	});

	const yAxisTicks = $derived.by(() => {
		const ticks: { value: number; y: number; label: string }[] = [];
		const tickCount = 5;
		for (let i = 0; i <= tickCount; i++) {
			const value = yExtent[0] + (yExtent[1] - yExtent[0]) * (i / tickCount);
			const y = chartHeight - (chartHeight / tickCount) * i;
			ticks.push({ value, y, label: formatY(value) });
		}
		return ticks;
	});
</script>

<div class="relative">
	{#if title}
		<h3 class="text-lg font-medium text-text mb-4 text-center">{title}</h3>
	{/if}

	<svg {width} {height} class="border border-border rounded-lg bg-bg">
		<!-- Chart area background -->
		<rect
			x={padding.left}
			y={padding.top}
			width={chartWidth}
			height={chartHeight}
			fill="var(--color-bg)"
			stroke="none"
		/>

		<!-- Grid lines -->
		{#if showGrid}
			<g transform={`translate(${padding.left}, ${padding.top})`}>
				{#each gridLines as line}
					<line
						x1={line.x1}
						y1={line.y1}
						x2={line.x2}
						y2={line.y2}
						stroke="var(--color-border)"
						stroke-width="1"
						stroke-dasharray="2,2"
					/>
				{/each}
			</g>
		{/if}

		<!-- Chart content -->
		<g transform={`translate(${padding.left}, ${padding.top})`}>
			{#if type === 'area'}
				<!-- Area fill -->
				<path
					d={areaPath}
					fill={color}
					fill-opacity="0.2"
					stroke="none"
				/>
			{/if}

			{#if type === 'line' || type === 'area'}
				<!-- Line -->
				<path
					d={linePath}
					fill="none"
					stroke={color}
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				/>

				<!-- Data points -->
				{#if showDots}
					{#each data as point}
						<circle
							cx={xScale(point.x)}
							cy={yScale(point.y)}
							r="4"
							fill={color}
							stroke="white"
							stroke-width="2"
						/>
					{/each}
				{/if}
			{:else if type === 'bar'}
				<!-- Bars -->
				{#each data as point, i}
					{@const barWidth = chartWidth / data.length * 0.8}
					{@const barX = (chartWidth / data.length) * i + (chartWidth / data.length - barWidth) / 2}
					{@const barHeight = yScale(yExtent[0]) - yScale(point.y)}
					<rect
						x={barX}
						y={yScale(point.y)}
						width={barWidth}
						height={barHeight}
						fill={color}
						rx="2"
					/>
				{/each}
			{/if}
		</g>

		<!-- X-axis -->
		<g transform={`translate(${padding.left}, ${padding.top + chartHeight})`}>
			<line x1="0" y1="0" x2={chartWidth} y2="0" stroke="var(--color-text-secondary)" stroke-width="1"/>
			{#each xAxisTicks as tick}
				<g transform={`translate(${tick.x}, 0)`}>
					<line x1="0" y1="0" x2="0" y2="6" stroke="var(--color-text-secondary)" stroke-width="1"/>
					<text
						x="0"
						y="20"
						text-anchor="middle"
						class="text-xs fill-text-secondary"
					>
						{tick.label}
					</text>
				</g>
			{/each}
			{#if xLabel}
				<text
					x={chartWidth / 2}
					y="50"
					text-anchor="middle"
					class="text-sm fill-text font-medium"
				>
					{xLabel}
				</text>
			{/if}
		</g>

		<!-- Y-axis -->
		<g transform={`translate(${padding.left}, ${padding.top})`}>
			<line x1="0" y1="0" x2="0" y2={chartHeight} stroke="var(--color-text-secondary)" stroke-width="1"/>
			{#each yAxisTicks as tick}
				<g transform={`translate(0, ${tick.y})`}>
					<line x1="-6" y1="0" x2="0" y2="0" stroke="var(--color-text-secondary)" stroke-width="1"/>
					<text
						x="-10"
						y="0"
						text-anchor="end"
						dominant-baseline="middle"
						class="text-xs fill-text-secondary"
					>
						{tick.label}
					</text>
				</g>
			{/each}
			{#if yLabel}
				<text
					x="-50"
					y={chartHeight / 2}
					text-anchor="middle"
					dominant-baseline="middle"
					transform={`rotate(-90, -50, ${chartHeight / 2})`}
					class="text-sm fill-text font-medium"
				>
					{yLabel}
				</text>
			{/if}
		</g>
	</svg>

	{#if data.length === 0}
		<div class="absolute inset-0 flex items-center justify-center">
			<div class="text-center">
				<svg class="mx-auto h-12 w-12 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
				</svg>
				<h3 class="mt-2 text-sm font-medium text-text">No data</h3>
				<p class="mt-1 text-sm text-text-muted">No data points to display</p>
			</div>
		</div>
	{/if}
</div>

