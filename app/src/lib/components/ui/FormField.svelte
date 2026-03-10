<!--
  Form Field Wrapper Component
  Unified wrapper for label + input + helper text + error message
-->

<script lang="ts">
	interface Props {
		label?: string;
		id?: string;
		error?: string;
		hint?: string;
		required?: boolean;
		disabled?: boolean;
		class?: string;
		children: any;
	}

	let {
		label,
		id,
		error,
		hint,
		required = false,
		disabled = false,
		class: extraClass = '',
		children
	}: Props = $props();

	const fieldId = $derived(id || (label ? label.toLowerCase().replace(/\s+/g, '-') : undefined));
	const hasError = $derived(!!error);
</script>

<div class="form-control w-full {extraClass}" class:opacity-50={disabled}>
	{#if label}
		<label class="label" for={fieldId}>
			<span class="label-text font-medium">
				{label}
				{#if required}
					<span class="text-error">*</span>
				{/if}
			</span>
		</label>
	{/if}

	<div class:has-error={hasError}>
		{@render children()}
	</div>

	{#if error || hint}
		<label class="label">
			{#if error}
				<span class="label-text-alt text-error">{error}</span>
			{:else if hint}
				<span class="label-text-alt text-base-content/60">{hint}</span>
			{/if}
		</label>
	{/if}
</div>
