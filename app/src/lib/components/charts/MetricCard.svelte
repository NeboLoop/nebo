<script lang="ts">
	import { TrendingUp, TrendingDown, Minus } from 'lucide-svelte';
	import type { MetricData } from './types';

	interface Props {
		title: string;
		value: string | number;
		trend?: number;
		trendPeriod?: string;
		format?: 'number' | 'currency' | 'percentage' | 'duration';
		icon?: any;
		color?: string;
		description?: string;
		loading?: boolean;
	}

	let {
		title,
		value,
		trend,
		trendPeriod = 'vs last period',
		format = 'number',
		icon,
		color = 'indigo',
		description,
		loading = false
	}: Props = $props();

	const formattedValue = $derived.by(() => {
		if (loading) return '...';
		if (value === undefined || value === null) return '0';

		if (typeof value === 'string') return value;

		switch (format) {
			case 'currency':
				return new Intl.NumberFormat('en-US', {
					style: 'currency',
					currency: 'USD',
					minimumFractionDigits: 0,
					maximumFractionDigits: 2
				}).format(value);
			case 'percentage':
				return `${value.toFixed(1)}%`;
			case 'duration':
				// Assume value is in seconds
				if (value < 60) return `${value.toFixed(0)}s`;
				if (value < 3600) return `${(value / 60).toFixed(1)}m`;
				return `${(value / 3600).toFixed(1)}h`;
			case 'number':
			default:
				if (value >= 1000000) {
					return `${(value / 1000000).toFixed(2)}M`;
				} else if (value >= 1000) {
					return `${(value / 1000).toFixed(1)}K`;
				}
				return value.toLocaleString();
		}
	});

	const trendDirection = $derived.by(() => {
		if (trend === undefined || trend === 0) return 'neutral';
		return trend > 0 ? 'up' : 'down';
	});

	const trendColor = $derived.by(() => {
		if (trendDirection === 'neutral') return 'text-base-500';
		return trendDirection === 'up' ? 'text-success' : 'text-error';
	});

	const colorClasses = $derived.by(() => {
		const colors: Record<string, string> = {
			indigo: 'from-indigo-600 to-indigo-500',
			purple: 'from-purple-600 to-purple-500',
			pink: 'from-pink-600 to-pink-500',
			orange: 'from-orange-600 to-orange-500',
			green: 'from-green-600 to-green-500',
			blue: 'from-blue-600 to-blue-500',
			red: 'from-red-600 to-red-500',
			yellow: 'from-yellow-600 to-yellow-500'
		};
		return colors[color] || colors.indigo;
	});
</script>

<div class="card group relative overflow-hidden">
	<!-- Title -->
	<p class="text-sm font-medium text-text-muted mb-2">{title}</p>

	<!-- Value -->
	{#if loading}
		<div class="skeleton h-9 w-24 mb-1"></div>
	{:else}
		<p class="text-3xl font-bold text-text transition-all duration-500 group-hover:scale-105 origin-left">
			{formattedValue}
		</p>
	{/if}

	<!-- Description (below value) -->
	{#if description}
		<p class="text-xs text-text-muted/70 mt-1">{description}</p>
	{/if}

	<!-- Trend -->
	{#if trend !== undefined}
		<div class="flex items-center gap-2 text-sm mt-2">
			{#if trendDirection === 'up'}
				<div class="flex items-center gap-1 {trendColor}">
					<TrendingUp class="w-4 h-4" />
					<span class="font-semibold">{Math.abs(trend).toFixed(1)}%</span>
				</div>
			{:else if trendDirection === 'down'}
				<div class="flex items-center gap-1 {trendColor}">
					<TrendingDown class="w-4 h-4" />
					<span class="font-semibold">{Math.abs(trend).toFixed(1)}%</span>
				</div>
			{:else}
				<div class="flex items-center gap-1 {trendColor}">
					<Minus class="w-4 h-4" />
					<span class="font-semibold">0%</span>
				</div>
			{/if}
			<span class="text-text-muted text-xs">{trendPeriod}</span>
		</div>
	{/if}

	<!-- Decorative gradient border on hover -->
	<div class="absolute inset-0 rounded-xl bg-gradient-to-r {colorClasses} opacity-0 group-hover:opacity-10 transition-opacity duration-300 pointer-events-none"></div>
</div>
