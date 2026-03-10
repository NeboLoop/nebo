<!--
  Checkbox Component
  Standard checkbox with optional label and description
-->

<script lang="ts">
	interface Props {
		checked?: boolean;
		disabled?: boolean;
		label?: string;
		description?: string;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		onchange?: (checked: boolean) => void;
		id?: string;
		class?: string;
	}

	let {
		checked = $bindable(),
		disabled = false,
		label,
		description,
		size = 'md',
		onchange,
		id,
		class: extraClass = ''
	}: Props = $props();

	const checkboxId = $derived(id || `checkbox-${Math.random().toString(36).slice(2, 9)}`);
	let safeChecked = $derived(checked ?? false);

	const sizeClasses: Record<string, string> = {
		xs: 'checkbox-xs',
		sm: 'checkbox-sm',
		md: 'checkbox-md',
		lg: 'checkbox-lg'
	};

	function handleChange(e: Event) {
		const target = e.currentTarget as HTMLInputElement;
		checked = target.checked;
		onchange?.(checked);
	}
</script>

<div class="form-control">
	<label class="label cursor-pointer justify-start gap-3" for={checkboxId}>
		<input
			type="checkbox"
			id={checkboxId}
			checked={safeChecked}
			{disabled}
			onchange={handleChange}
			class="checkbox checkbox-primary {sizeClasses[size]} {extraClass}"
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
