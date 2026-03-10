<!--
  Slider/Range Component
  For selecting numeric values with a slider
-->

<script lang="ts">
	interface Props {
		value?: number;
		min?: number;
		max?: number;
		step?: number;
		disabled?: boolean;
		label?: string;
		showValue?: boolean;
		formatValue?: (value: number) => string;
		onchange?: (value: number) => void;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		id?: string;
		class?: string;
	}

	let {
		value = $bindable(50),
		min = 0,
		max = 100,
		step = 1,
		disabled = false,
		label,
		showValue = true,
		formatValue = (v) => v.toString(),
		onchange,
		size = 'md',
		id,
		class: extraClass = ''
	}: Props = $props();

	const sliderId = $derived(id || `slider-${Math.random().toString(36).slice(2, 9)}`);

	const sizeClasses: Record<string, string> = {
		xs: 'range-xs',
		sm: 'range-sm',
		md: 'range-md',
		lg: 'range-lg'
	};

	function handleChange(event: Event) {
		const target = event.target as HTMLInputElement;
		value = Number(target.value);
		onchange?.(value);
	}
</script>

<div class="w-full">
	{#if label}
		<div class="mb-2 flex items-center justify-between">
			<label for={sliderId} class="label-text font-medium">{label}</label>
			{#if showValue}
				<span class="label-text-alt text-primary font-semibold">{formatValue(value)}</span>
			{/if}
		</div>
	{/if}
	<input
		type="range"
		id={sliderId}
		bind:value
		{min}
		{max}
		{step}
		{disabled}
		onchange={handleChange}
		oninput={handleChange}
		class="range range-primary w-full {sizeClasses[size]} {extraClass}"
	/>
</div>
