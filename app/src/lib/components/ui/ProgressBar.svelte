<!--
  Progress Bar Component
  Uses DaisyUI progress classes
-->

<script lang="ts">
	interface Props {
		value: number;
		max?: number;
		label?: string;
		showPercent?: boolean;
		size?: 'sm' | 'md' | 'lg';
		variant?: 'default' | 'success' | 'warning' | 'error';
		striped?: boolean;
		animated?: boolean;
	}

	let {
		value,
		max = 100,
		label,
		showPercent = true,
		size = 'md',
		variant = 'default',
		striped = false,
		animated = false
	}: Props = $props();

	let percentage = $derived(Math.min(100, Math.max(0, (value / max) * 100)));

	const variantClass = $derived({
		default: 'progress-primary',
		success: 'progress-success',
		warning: 'progress-warning',
		error: 'progress-error'
	}[variant]);
</script>

<div class="w-full">
	{#if label || showPercent}
		<div class="mb-2 flex items-center justify-between">
			{#if label}
				<span class="text-sm font-medium text-base-content/70">{label}</span>
			{/if}
			{#if showPercent}
				<span class="text-sm font-semibold">{percentage.toFixed(0)}%</span>
			{/if}
		</div>
	{/if}

	<progress
		class="progress {variantClass} w-full"
		{value}
		{max}
		aria-valuenow={value}
		aria-valuemin={0}
		aria-valuemax={max}
	></progress>
</div>
