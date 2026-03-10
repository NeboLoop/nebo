<!--
  Radio Component
  Styled radio button with label
-->

<script lang="ts">
	interface Props {
		value: string;
		checked?: boolean;
		name: string;
		disabled?: boolean;
		label?: string;
		description?: string;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		onchange?: (value: string) => void;
		id?: string;
		class?: string;
	}

	let {
		value,
		checked = false,
		name,
		disabled = false,
		label,
		description,
		size = 'md',
		onchange,
		id,
		class: extraClass = ''
	}: Props = $props();

	const radioId = $derived(id || `radio-${Math.random().toString(36).slice(2, 9)}`);

	const sizeClasses: Record<string, string> = {
		xs: 'radio-xs',
		sm: 'radio-sm',
		md: 'radio-md',
		lg: 'radio-lg'
	};

	function handleChange() {
		onchange?.(value);
	}
</script>

<div class="form-control">
	<label class="label cursor-pointer justify-start gap-3" for={radioId}>
		<input
			type="radio"
			id={radioId}
			{name}
			{value}
			{checked}
			{disabled}
			onchange={handleChange}
			class="radio radio-primary {sizeClasses[size]} {extraClass}"
		/>
		{#if label || description}
			<div class="flex flex-col">
				{#if label}
					<span class="label-text {disabled ? 'opacity-50' : ''}">{label}</span>
				{/if}
				{#if description}
					<span class="label-text-alt text-base-content/60 {disabled ? 'opacity-50' : ''}">{description}</span>
				{/if}
			</div>
		{/if}
	</label>
</div>
