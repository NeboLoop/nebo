<!--
  Toggle/Switch Component
  For boolean on/off settings
-->

<script lang="ts">
	interface Props {
		checked?: boolean;
		disabled?: boolean;
		label?: string;
		description?: string;
		onchange?: (checked: boolean) => void;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		id?: string;
		class?: string;
	}

	let {
		checked = $bindable(false),
		disabled = false,
		label,
		description,
		onchange,
		size = 'md',
		id,
		class: extraClass = ''
	}: Props = $props();

	const toggleId = $derived(id || `toggle-${Math.random().toString(36).slice(2, 9)}`);

	const sizeClasses: Record<string, string> = {
		xs: 'toggle-xs',
		sm: 'toggle-sm',
		md: 'toggle-md',
		lg: 'toggle-lg'
	};

	function handleChange(e: Event) {
		const target = e.currentTarget as HTMLInputElement;
		checked = target.checked;
		onchange?.(checked);
	}
</script>

<div class="form-control">
	<label class="label cursor-pointer justify-start gap-3" for={toggleId}>
		<input
			type="checkbox"
			id={toggleId}
			checked={checked}
			{disabled}
			onchange={handleChange}
			class="toggle toggle-primary {sizeClasses[size]} {extraClass}"
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
