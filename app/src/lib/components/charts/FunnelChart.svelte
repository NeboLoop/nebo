<script lang="ts">
	import { onMount } from 'svelte';
	import ChartTooltip from './ChartTooltip.svelte';

	interface FunnelStage {
		label: string;
		value: number;
		color?: string;
	}

	interface Props {
		data: FunnelStage[] | { data: FunnelStage[] } | { stages: FunnelStage[] };
		height?: number;
		orientation?: 'vertical' | 'horizontal';
		showLabels?: boolean;
		showValues?: boolean;
		showPercentages?: boolean;
		onStageClick?: (data: { label: string; value: number; percentage: number; conversionRate: number; index: number }) => void;
	}

	let {
		data: rawData = [],
		height,
		orientation = 'vertical',
		showLabels = true,
		showValues = true,
		showPercentages = true,
		onStageClick
	}: Props = $props();

	// Normalize data - handle direct array, { data: [...] }, or { stages: [...] } structure
	const data = $derived.by<FunnelStage[]>(() => {
		if (Array.isArray(rawData)) {
			return rawData;
		}
		if (rawData && typeof rawData === 'object') {
			if ('data' in rawData && Array.isArray(rawData.data)) {
				return rawData.data;
			}
			if ('stages' in rawData && Array.isArray(rawData.stages)) {
				return rawData.stages;
			}
		}
		return [];
	});

	let containerRef = $state<HTMLDivElement | undefined>(undefined);
	let containerWidth = $state(400);
	let containerHeight = $state(400);

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
	let hoveredStage = $state<number | null>(null);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					containerWidth = Math.max(entry.contentRect.width, 200);
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

	const defaultColors = [
		'rgba(99, 102, 241, 0.85)',
		'rgba(129, 140, 248, 0.85)',
		'rgba(165, 180, 252, 0.85)',
		'rgba(199, 210, 254, 0.85)',
		'rgba(224, 231, 255, 0.85)',
	];

	const maxValue = $derived(data.length > 0 ? Math.max(...data.map(d => d.value)) : 1);
	const firstValue = $derived(data.length > 0 ? data[0].value : 1);

	function getColor(index: number, stage: FunnelStage): string {
		return stage.color || defaultColors[index % defaultColors.length];
	}

	function formatValue(value: number): string {
		if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
		if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
		return value.toLocaleString();
	}

	function getPercentage(value: number): string {
		return `${((value / firstValue) * 100).toFixed(1)}%`;
	}

	function getPercentageNum(value: number): number {
		return (value / firstValue) * 100;
	}

	function getConversionRate(currentIndex: number): string {
		if (currentIndex === 0) return '100%';
		const prevValue = data[currentIndex - 1].value;
		const currentValue = data[currentIndex].value;
		if (prevValue === 0) return '0%';
		return `${((currentValue / prevValue) * 100).toFixed(1)}%`;
	}

	function getConversionRateNum(currentIndex: number): number {
		if (currentIndex === 0) return 100;
		const prevValue = data[currentIndex - 1].value;
		const currentValue = data[currentIndex].value;
		if (prevValue === 0) return 0;
		return (currentValue / prevValue) * 100;
	}

	// Calculate max label width for dynamic padding
	const maxLabelLength = $derived(
		data.length > 0 ? Math.max(...data.map(d => d.label?.length || 0)) : 10
	);

	// Dynamic right padding based on label length (approximate 6px per character + percentage text)
	const rightPadding = $derived(Math.max(140, Math.min(200, maxLabelLength * 7 + 80)));

	// Vertical funnel paths
	const verticalFunnelData = $derived.by(() => {
		if (orientation !== 'vertical' || data.length === 0) return [];

		const padding = { top: 20, right: rightPadding, bottom: 20, left: 40 };
		const availableWidth = containerWidth - padding.left - padding.right;
		const availableHeight = effectiveHeight - padding.top - padding.bottom;
		const stageHeight = availableHeight / data.length;
		const funnelCenterX = padding.left + availableWidth / 2;

		return data.map((stage, i) => {
			const topWidth = (stage.value / maxValue) * availableWidth;
			const nextValue = data[i + 1]?.value ?? stage.value * 0.5;
			const bottomWidth = (nextValue / maxValue) * availableWidth;

			const topY = padding.top + i * stageHeight;
			const bottomY = topY + stageHeight;

			const topLeft = funnelCenterX - topWidth / 2;
			const topRight = funnelCenterX + topWidth / 2;
			const bottomLeft = funnelCenterX - bottomWidth / 2;
			const bottomRight = funnelCenterX + bottomWidth / 2;

			// Trapezoid path
			const path = `M ${topLeft} ${topY} L ${topRight} ${topY} L ${bottomRight} ${bottomY} L ${bottomLeft} ${bottomY} Z`;

			return {
				path,
				stage,
				centerY: topY + stageHeight / 2,
				labelX: padding.left + availableWidth + 15,
				funnelCenterX
			};
		});
	});

	// Horizontal funnel paths
	const horizontalFunnelData = $derived.by(() => {
		if (orientation !== 'horizontal' || data.length === 0) return [];

		const padding = { top: 60, right: 20, bottom: 60, left: 20 };
		const availableWidth = containerWidth - padding.left - padding.right;
		const availableHeight = effectiveHeight - padding.top - padding.bottom;
		const stageWidth = availableWidth / data.length;
		const centerY = effectiveHeight / 2;

		return data.map((stage, i) => {
			const leftHeight = (stage.value / maxValue) * availableHeight;
			const nextValue = data[i + 1]?.value ?? stage.value * 0.5;
			const rightHeight = (nextValue / maxValue) * availableHeight;

			const leftX = padding.left + i * stageWidth;
			const rightX = leftX + stageWidth;

			const topLeft = centerY - leftHeight / 2;
			const bottomLeft = centerY + leftHeight / 2;
			const topRight = centerY - rightHeight / 2;
			const bottomRight = centerY + rightHeight / 2;

			// Trapezoid path (horizontal)
			const path = `M ${leftX} ${topLeft} L ${rightX} ${topRight} L ${rightX} ${bottomRight} L ${leftX} ${bottomLeft} Z`;

			return {
				path,
				stage,
				centerX: leftX + stageWidth / 2,
				labelY: effectiveHeight - padding.bottom + 20
			};
		});
	});

	function handleStageMouseEnter(event: MouseEvent, stage: FunnelStage, index: number) {
		hoveredStage = index;

		if (!containerRef) return;
		const rect = containerRef.getBoundingClientRect();
		tooltipX = event.clientX - rect.left;
		tooltipY = event.clientY - rect.top;

		tooltipData = {
			label: stage.label,
			value: stage.value,
			percentage: getPercentage(stage.value),
			color: getColor(index, stage),
			secondaryLabel: 'Conversion',
			secondaryValue: getConversionRate(index)
		};
		tooltipVisible = true;
	}

	function handleStageMouseMove(event: MouseEvent) {
		if (tooltipVisible && containerRef) {
			const rect = containerRef.getBoundingClientRect();
			tooltipX = event.clientX - rect.left;
			tooltipY = event.clientY - rect.top;
		}
	}

	function handleStageMouseLeave() {
		hoveredStage = null;
		tooltipVisible = false;
		tooltipData = null;
	}

	function handleStageClick(stage: FunnelStage, index: number) {
		if (onStageClick) {
			onStageClick({
				label: stage.label,
				value: stage.value,
				percentage: getPercentageNum(stage.value),
				conversionRate: getConversionRateNum(index),
				index
			});
		}
	}

	function isStageDimmed(index: number): boolean {
		if (hoveredStage === null) return false;
		return hoveredStage !== index;
	}

	function isStageHighlighted(index: number): boolean {
		if (hoveredStage === null) return false;
		return hoveredStage === index;
	}
</script>

<div bind:this={containerRef} class="flex flex-col w-full h-full relative">
	{#if data.length === 0}
		<div class="flex-1 flex items-center justify-center">
			<p class="text-base-500 text-sm">No data to display</p>
		</div>
	{:else}
		<svg width={containerWidth} height={effectiveHeight} class="flex-1">
			{#if orientation === 'vertical'}
				{#each verticalFunnelData as item, i}
					<g class="funnel-stage">
						<path
							d={item.path}
							fill={getColor(i, item.stage)}
							stroke="#1e293b"
							stroke-width="2"
							class="transition-all duration-150 cursor-pointer {isStageHighlighted(i) ? 'brightness-110' : ''} {isStageDimmed(i) ? 'opacity-40' : ''}"
							onmouseenter={(e) => handleStageMouseEnter(e, item.stage, i)}
							onmousemove={handleStageMouseMove}
							onmouseleave={handleStageMouseLeave}
							onclick={() => handleStageClick(item.stage, i)}
							role="button"
							tabindex="0"
							onkeydown={(e) => e.key === 'Enter' && handleStageClick(item.stage, i)}
						/>

						<!-- Center label -->
						{#if showValues}
							<text
								x={item.funnelCenterX}
								y={item.centerY}
								text-anchor="middle"
								dominant-baseline="middle"
								class="fill-white font-semibold text-sm pointer-events-none"
							>
								{formatValue(item.stage.value)}
							</text>
						{/if}

						<!-- Side label -->
						{#if showLabels}
							<text
								x={item.labelX}
								y={item.centerY - 10}
								text-anchor="start"
								class="fill-base-400 text-xs font-medium pointer-events-none"
							>
								{item.stage.label}
							</text>
							{#if showPercentages}
								<text
									x={item.labelX}
									y={item.centerY + 8}
									text-anchor="start"
									class="fill-base-500 text-xs pointer-events-none"
								>
									{getPercentage(item.stage.value)} of total - {getConversionRate(i)} conv.
								</text>
							{/if}
						{/if}
					</g>
				{/each}
			{:else}
				{#each horizontalFunnelData as item, i}
					<g class="funnel-stage">
						<path
							d={item.path}
							fill={getColor(i, item.stage)}
							stroke="#1e293b"
							stroke-width="2"
							class="transition-all duration-150 cursor-pointer {isStageHighlighted(i) ? 'brightness-110' : ''} {isStageDimmed(i) ? 'opacity-40' : ''}"
							onmouseenter={(e) => handleStageMouseEnter(e, item.stage, i)}
							onmousemove={handleStageMouseMove}
							onmouseleave={handleStageMouseLeave}
							onclick={() => handleStageClick(item.stage, i)}
							role="button"
							tabindex="0"
							onkeydown={(e) => e.key === 'Enter' && handleStageClick(item.stage, i)}
						/>

						<!-- Center label -->
						{#if showValues}
							<text
								x={item.centerX}
								y={effectiveHeight / 2}
								text-anchor="middle"
								dominant-baseline="middle"
								class="fill-white font-semibold text-sm pointer-events-none"
							>
								{formatValue(item.stage.value)}
							</text>
						{/if}

						<!-- Bottom label -->
						{#if showLabels}
							<text
								x={item.centerX}
								y={item.labelY}
								text-anchor="middle"
								class="fill-base-400 text-xs font-medium pointer-events-none"
							>
								{item.stage.label}
							</text>
							{#if showPercentages}
								<text
									x={item.centerX}
									y={item.labelY + 14}
									text-anchor="middle"
									class="fill-base-500 text-xs pointer-events-none"
								>
									{getPercentage(item.stage.value)}
								</text>
							{/if}
						{/if}
					</g>
				{/each}
			{/if}
		</svg>

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
