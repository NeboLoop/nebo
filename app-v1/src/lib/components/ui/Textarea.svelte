<!--
  Textarea Component
  Multi-line text input
-->

<script lang="ts">
	interface Props {
		id?: string;
		value?: string;
		placeholder?: string;
		disabled?: boolean;
		rows?: number;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		class?: string;
	}

	let {
		id,
		value = $bindable(''),
		placeholder = '',
		disabled = false,
		rows = 3,
		size = 'md',
		class: extraClass = ''
	}: Props = $props();

	const sizeClasses: Record<string, string> = {
		xs: 'textarea-xs',
		sm: 'textarea-sm',
		md: 'textarea-md',
		lg: 'textarea-lg'
	};

	const className = $derived(`textarea textarea-bordered w-full ${sizeClasses[size]} ${extraClass}`.trim());
</script>

<textarea
	{id}
	bind:value
	{placeholder}
	{disabled}
	{rows}
	class={className}
></textarea>
