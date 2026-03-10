<script lang="ts">
	import { SparklineChart } from '$lib/components/charts';

	interface Props {
		title?: string;
		value: string | number;
		change?: number;
		changeType?: 'increase' | 'decrease' | 'neutral';
		subtitle?: string;
		icon?: string;
		loading?: boolean;
		error?: string;
		format?: 'number' | 'currency' | 'percentage' | 'ratio';
		precision?: number;
		sparklineData?: number[];
		class?: string;
	}

	let {
		title,
		value,
		change,
		changeType = 'neutral',
		subtitle,
		icon,
		loading = false,
		error,
		format = 'number',
		precision = 2,
		sparklineData = [],
		class: className = ''
	}: Props = $props();

	function formatValue(val: string | number): string {
		if (typeof val === 'string') return val;
		if (val === undefined || val === null || typeof val !== 'number') return '0';

		switch (format) {
			case 'currency':
				return new Intl.NumberFormat('en-US', {
					style: 'currency',
					currency: 'USD',
					minimumFractionDigits: 0,
					maximumFractionDigits: precision
				}).format(val);
			case 'percentage':
				return `${(val * 100).toFixed(precision)}%`;
			case 'ratio':
				return `${val.toFixed(precision)}x`;
			default:
				return new Intl.NumberFormat('en-US', {
					minimumFractionDigits: 0,
					maximumFractionDigits: precision
				}).format(val);
		}
	}

	function formatChange(val: number | undefined): string {
		if (val === undefined || val === null || typeof val !== 'number') {
			return '0.0%';
		}
		const sign = val >= 0 ? '+' : '';
		return `${sign}${val.toFixed(1)}%`;
	}

	let sparklineColor = $derived.by(() => {
		if (changeType === 'increase') return 'var(--color-success)';
		if (changeType === 'decrease') return 'var(--color-error)';
		return 'var(--color-base-400)';
	});

	let cardBorderClass = $derived.by(() => {
		if (changeType === 'increase') return 'border-success/30 hover:border-success/50';
		if (changeType === 'decrease') return 'border-error/30 hover:border-error/50';
		return 'border-base-300 hover:border-base-content/20';
	});

	let changeBadgeClass = $derived.by(() => {
		if (changeType === 'increase') return 'badge-success';
		if (changeType === 'decrease') return 'badge-error';
		return 'badge-ghost';
	});
</script>

<div class="w-full h-full {className}">
	{#if loading}
		<div class="card bg-base-200 border {cardBorderClass} h-full">
			<div class="card-body p-5">
				<div class="flex flex-col gap-3">
					{#if title}
						<div class="skeleton h-3 w-24"></div>
					{/if}
					<div class="skeleton h-8 w-24"></div>
					<div class="skeleton h-2 w-20"></div>
				</div>
			</div>
		</div>
	{:else if error}
		<div class="card bg-base-200 border border-error/30 h-full">
			<div class="card-body p-4 items-center justify-center text-center">
				{#if title}
					<div class="text-[10px] font-semibold text-base-content/60 uppercase tracking-wider mb-2">{title}</div>
				{/if}
				<i class="fas fa-exclamation-triangle text-3xl text-error mb-2"></i>
				<div class="text-xs text-error">{error}</div>
			</div>
		</div>
	{:else}
		<div class="card bg-base-200 border {cardBorderClass} h-full transition-all duration-200">
			<div class="card-body p-5 flex flex-col">
				<!-- Title Row -->
				{#if title}
					<div class="text-[10px] font-semibold text-base-content/60 uppercase tracking-wider mb-3">{title}</div>
				{/if}

				<!-- Main Value & Change -->
				<div class="mb-4">
					<div class="flex items-end gap-3 mb-1">
						<div class="text-4xl font-bold text-base-content tracking-tight leading-none">
							{formatValue(value)}
						</div>
						{#if change !== undefined}
							<div class="badge {changeBadgeClass} badge-sm gap-1 mb-1">
								{#if changeType === 'increase'}
									<svg class="h-3 w-3" fill="currentColor" viewBox="0 0 20 20">
										<path fill-rule="evenodd" d="M5.293 9.707a1 1 0 010-1.414l4-4a1 1 0 011.414 0l4 4a1 1 0 01-1.414 1.414L11 7.414V15a1 1 0 11-2 0V7.414L6.707 9.707a1 1 0 01-1.414 0z" clip-rule="evenodd" />
									</svg>
								{:else if changeType === 'decrease'}
									<svg class="h-3 w-3" fill="currentColor" viewBox="0 0 20 20">
										<path fill-rule="evenodd" d="M14.707 10.293a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0l-4-4a1 1 0 111.414-1.414L9 12.586V5a1 1 0 012 0v7.586l2.293-2.293a1 1 0 011.414 0z" clip-rule="evenodd" />
									</svg>
								{/if}
								<span class="text-xs font-bold">{formatChange(change)}</span>
							</div>
						{/if}
					</div>
					{#if subtitle}
						<div class="text-xs text-base-content/60">{subtitle}</div>
					{/if}
				</div>

				<!-- Large Sparkline Chart (takes remaining space) -->
				{#if sparklineData && sparklineData.length > 0}
					<div class="flex-1 min-h-[80px] -mb-2">
						<SparklineChart
							data={sparklineData}
							width={300}
							height={80}
							color={sparklineColor}
							fill={true}
						/>
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>
