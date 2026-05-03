<script lang="ts">
	interface Props {
		data: number[];
		width?: number;
		height?: number;
		color?: string;
		showArea?: boolean;
	}

	let {
		data,
		width = 100,
		height = 30,
		color = 'currentColor',
		showArea = true
	}: Props = $props();

	// Calculate SVG path from data points
	let path = $derived.by(() => {
		if (!data || data.length === 0) return '';

		const min = Math.min(...data);
		const max = Math.max(...data);
		const range = max - min || 1;

		const points = data.map((value, index) => {
			const x = (index / (data.length - 1)) * width;
			const y = height - ((value - min) / range) * height;
			return `${x},${y}`;
		});

		return `M ${points.join(' L ')}`;
	});

	// Area fill path (extends to bottom)
	let areaPath = $derived.by(() => {
		if (!data || data.length === 0) return '';

		const min = Math.min(...data);
		const max = Math.max(...data);
		const range = max - min || 1;

		const points = data.map((value, index) => {
			const x = (index / (data.length - 1)) * width;
			const y = height - ((value - min) / range) * height;
			return `${x},${y}`;
		});

		return `M 0,${height} L ${points.join(' L ')} L ${width},${height} Z`;
	});
</script>

<svg class="block w-full h-full" viewBox="0 0 {width} {height}" preserveAspectRatio="none">
	<!-- Subtle grid lines -->
	<line x1="0" y1="{height / 2}" x2="{width}" y2="{height / 2}" stroke="currentColor" stroke-width="0.5" opacity="0.1" />
	<line x1="0" y1="{height}" x2="{width}" y2="{height}" stroke="currentColor" stroke-width="0.5" opacity="0.15" />

	{#if showArea && areaPath}
		<path
			d={areaPath}
			fill={color}
			opacity="0.25"
		/>
	{/if}
	{#if path}
		<path
			d={path}
			fill="none"
			stroke={color}
			stroke-width="2.5"
			stroke-linecap="round"
			stroke-linejoin="round"
		/>
	{/if}
</svg>

