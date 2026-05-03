<!--
  Reusable Input Component
-->

<script lang="ts">
	let {
		type = 'text',
		size = 'md',
		placeholder = '',
		value = $bindable(),
		disabled = false,
		readonly = false,
		required = false,
		id,
		label,
		min,
		max,
		step,
		class: extraClass = '',
		oninput,
		onblur,
		onfocus
	}: {
		type?: string;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		placeholder?: string;
		value?: string | number;
		disabled?: boolean;
		readonly?: boolean;
		required?: boolean;
		id?: string;
		label?: string;
		min?: string | number;
		max?: string | number;
		step?: string | number;
		class?: string;
		oninput?: (e: Event) => void;
		onblur?: (e: Event) => void;
		onfocus?: (e: Event) => void;
	} = $props();

	// Ensure value is never undefined
	let safeValue = $derived(value ?? '');

	const sizeClasses: Record<string, string> = {
		xs: 'input-xs',
		sm: 'input-sm',
		md: 'input-md',
		lg: 'input-lg'
	};

	const className = $derived(`input input-bordered w-full ${sizeClasses[size]} ${extraClass}`.trim());

	function handleInput(e: Event) {
		const inputEl = e.currentTarget as HTMLInputElement;
		if (type === 'number') {
			value = inputEl.valueAsNumber;
		} else {
			value = inputEl.value;
		}
		oninput?.(e);
	}
</script>

{#if label}
	<label class="label" for={id}>
		<span class="label-text">{label}</span>
	</label>
{/if}
<input class={className} {type} {placeholder} value={safeValue} oninput={handleInput} onblur={onblur} onfocus={onfocus} {disabled} {readonly} {required} {id} {min} {max} {step} />
