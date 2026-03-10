<script lang="ts">
	import { onMount } from 'svelte';

	interface GaugeData {
		value: number;
		min?: number;
		max?: number;
		target?: number;
		label?: string;
		unit?: string;
		thresholds?: Array<{ value: number; color: string }>;
	}

	interface Props {
		data?: GaugeData;
		value?: number;
		min?: number;
		max?: number;
		target?: number;
		label?: string;
		unit?: string;
		height?: number;
		thresholds?: Array<{ value: number; color: string }>;
	}

	let {
		data,
		value: propValue,
		min: propMin = 0,
		max: propMax = 100,
		target: propTarget,
		label: propLabel,
		unit: propUnit = '',
		height,
		thresholds: propThresholds
	}: Props = $props();

	// Extract values from data object or use direct props
	const value = $derived(data?.value ?? propValue ?? 0);
	const min = $derived(data?.min ?? propMin);
	const max = $derived(data?.max ?? propMax);
	const target = $derived(data?.target ?? propTarget);
	const label = $derived(data?.label ?? propLabel);
	const unit = $derived(data?.unit ?? propUnit);
	const thresholds = $derived(data?.thresholds ?? propThresholds);

	let containerRef: HTMLDivElement;
	let containerSize = $state(200);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					const minDim = Math.min(entry.contentRect.width, entry.contentRect.height);
					containerSize = Math.max(minDim, 150);
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	const size = $derived(height || containerSize);
	const cx = $derived(size / 2);
	const cy = $derived(size * 0.55);
	const radius = $derived((size / 2) - 20);

	// Default thresholds if not provided
	const defaultThresholds = [
		{ value: 33, color: 'rgba(239, 68, 68, 0.85)' },    // Red
		{ value: 66, color: 'rgba(251, 191, 36, 0.85)' },   // Yellow
		{ value: 100, color: 'rgba(34, 197, 94, 0.85)' }    // Green
	];

	const activeThresholds = $derived(thresholds || defaultThresholds);

	// Gauge arc starts at -135 degrees and ends at 135 degrees (270 degree sweep)
	const startAngle = -135;
	const endAngle = 135;
	const sweepAngle = endAngle - startAngle;

	function valueToAngle(val: number): number {
		const normalizedValue = (val - min) / (max - min);
		const clampedValue = Math.max(0, Math.min(1, normalizedValue));
		return startAngle + clampedValue * sweepAngle;
	}

	function polarToCartesian(centerX: number, centerY: number, r: number, angleInDegrees: number): { x: number; y: number } {
		const angleInRadians = (angleInDegrees * Math.PI) / 180;
		return {
			x: centerX + r * Math.cos(angleInRadians),
			y: centerY + r * Math.sin(angleInRadians)
		};
	}

	function describeArc(x: number, y: number, r: number, startAng: number, endAng: number): string {
		const start = polarToCartesian(x, y, r, endAng);
		const end = polarToCartesian(x, y, r, startAng);
		const largeArcFlag = endAng - startAng <= 180 ? '0' : '1';
		return `M ${start.x} ${start.y} A ${r} ${r} 0 ${largeArcFlag} 0 ${end.x} ${end.y}`;
	}

	// Calculate arc segments for thresholds
	const arcSegments = $derived.by(() => {
		const segments: Array<{ path: string; color: string }> = [];
		let prevValue = min;

		for (const threshold of activeThresholds) {
			const thresholdValue = min + (threshold.value / 100) * (max - min);
			const segmentStart = valueToAngle(prevValue);
			const segmentEnd = valueToAngle(thresholdValue);

			segments.push({
				path: describeArc(cx, cy, radius, segmentStart, segmentEnd),
				color: threshold.color
			});

			prevValue = thresholdValue;
		}

		return segments;
	});

	// Value arc
	const valueArc = $derived(describeArc(cx, cy, radius - 15, startAngle, valueToAngle(value)));

	// Needle position
	const needleAngle = $derived(valueToAngle(value));
	const needleLength = $derived(radius - 25);
	const needleTip = $derived(polarToCartesian(cx, cy, needleLength, needleAngle));

	// Target marker position
	const targetMarker = $derived.by(() => {
		if (target === undefined) return null;
		const angle = valueToAngle(target);
		const outer = polarToCartesian(cx, cy, radius + 5, angle);
		const inner = polarToCartesian(cx, cy, radius - 20, angle);
		return { outer, inner };
	});

	function formatValue(val: number): string {
		if (Math.abs(val) >= 1000000) return `${(val / 1000000).toFixed(1)}M`;
		if (Math.abs(val) >= 1000) return `${(val / 1000).toFixed(1)}K`;
		return val.toFixed(0);
	}

	function getValueColor(): string {
		const normalizedValue = ((value - min) / (max - min)) * 100;
		for (const threshold of activeThresholds) {
			if (normalizedValue <= threshold.value) {
				return threshold.color;
			}
		}
		return activeThresholds[activeThresholds.length - 1]?.color || '#94a3b8';
	}
</script>

<div bind:this={containerRef} class="gauge-chart-container flex flex-col items-center justify-center w-full h-full">
	<svg width={size} height={size * 0.7} viewBox="0 0 {size} {size * 0.7}" class="overflow-visible">
		<!-- Background arcs (threshold segments) -->
		{#each arcSegments as segment}
			<path
				d={segment.path}
				fill="none"
				stroke={segment.color}
				stroke-width="20"
				stroke-linecap="round"
				opacity="0.3"
			/>
		{/each}

		<!-- Value arc -->
		<path
			d={valueArc}
			fill="none"
			stroke={getValueColor()}
			stroke-width="12"
			stroke-linecap="round"
			class="transition-all duration-500"
		/>

		<!-- Needle -->
		<line
			x1={cx}
			y1={cy}
			x2={needleTip.x}
			y2={needleTip.y}
			stroke="#f8fafc"
			stroke-width="3"
			stroke-linecap="round"
			class="transition-all duration-500"
		/>
		<circle
			cx={cx}
			cy={cy}
			r="8"
			fill="#f8fafc"
			stroke="#1e293b"
			stroke-width="2"
		/>

		<!-- Target marker -->
		{#if targetMarker}
			<line
				x1={targetMarker.inner.x}
				y1={targetMarker.inner.y}
				x2={targetMarker.outer.x}
				y2={targetMarker.outer.y}
				stroke="#f8fafc"
				stroke-width="3"
				stroke-linecap="round"
			/>
		{/if}

		<!-- Min/Max labels -->
		<text
			x={polarToCartesian(cx, cy, radius + 15, startAngle).x}
			y={polarToCartesian(cx, cy, radius + 15, startAngle).y + 5}
			text-anchor="middle"
			class="text-xs fill-base-content/60"
		>
			{formatValue(min)}
		</text>
		<text
			x={polarToCartesian(cx, cy, radius + 15, endAngle).x}
			y={polarToCartesian(cx, cy, radius + 15, endAngle).y + 5}
			text-anchor="middle"
			class="text-xs fill-base-content/60"
		>
			{formatValue(max)}
		</text>

		<!-- Center value -->
		<text
			x={cx}
			y={cy + 35}
			text-anchor="middle"
			class="fill-white font-bold"
			font-size="24"
		>
			{formatValue(value)}{unit}
		</text>

		<!-- Label -->
		{#if label}
			<text
				x={cx}
				y={cy + 55}
				text-anchor="middle"
				class="fill-base-content/60 text-sm"
			>
				{label}
			</text>
		{/if}

		<!-- Target label -->
		{#if target !== undefined}
			<text
				x={cx}
				y={cy + 75}
				text-anchor="middle"
				class="fill-base-content/60 text-xs"
			>
				Target: {formatValue(target)}{unit}
			</text>
		{/if}
	</svg>
</div>
