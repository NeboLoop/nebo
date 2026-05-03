<!--
  Reusable Select Component
-->

<script lang="ts">
	type Option = { value: string | number; label: string };

	let {
		value = $bindable(),
		size = 'md',
		disabled = false,
		options,
		children,
		id,
		class: extraClass = ''
	}: {
		value?: string | number;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		disabled?: boolean;
		options?: Option[];
		children?: any;
		id?: string;
		class?: string;
	} = $props();

	let safeValue = $derived(value ?? '');

	const sizeClasses: Record<string, string> = {
		xs: 'select-xs',
		sm: 'select-sm',
		md: 'select-md',
		lg: 'select-lg'
	};

	const className = $derived(`select select-bordered w-full ${sizeClasses[size]} ${extraClass}`.trim());
</script>

<select class={className} value={safeValue} onchange={(e) => value = e.currentTarget.value} {disabled} {id}>
	{#if options}
		{#each options as opt}
			<option value={opt.value}>{opt.label}</option>
		{/each}
	{:else}
		{@render children?.()}
	{/if}
</select>
