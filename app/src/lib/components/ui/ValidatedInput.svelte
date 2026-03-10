<!--
  ValidatedInput Component
  Input field with real-time validation feedback
-->

<script lang="ts">
	interface FieldValidation<T> {
		validate: (value: T, context?: any) => ValidationResult;
		showOn?: 'change' | 'blur';
		debounceMs?: number;
	}

	interface ValidationResult {
		isValid: boolean;
		hint?: string;
		warning?: string;
		severity?: 'warning' | 'error';
	}

	interface Props {
		value?: string;
		validation?: FieldValidation<string>;
		context?: any;
		showValidation?: boolean;
		forceValidate?: boolean;
		type?: string;
		placeholder?: string;
		disabled?: boolean;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		class?: string;
		onBlur?: (e: FocusEvent & { currentTarget: HTMLInputElement }) => void;
	}

	let {
		value = $bindable(''),
		validation,
		context,
		showValidation = true,
		forceValidate = false,
		type = 'text',
		placeholder = '',
		disabled = false,
		size = 'md',
		class: extraClass = '',
		onBlur
	}: Props = $props();

	let validationResult = $state<ValidationResult | null>(null);
	let shouldShow = $state(false);
	let debounceTimer: ReturnType<typeof setTimeout> | undefined = $state();
	let showTimer: ReturnType<typeof setTimeout> | undefined = $state();

	const sizeClasses: Record<string, string> = {
		xs: 'input-xs',
		sm: 'input-sm',
		md: 'input-md',
		lg: 'input-lg'
	};

	// Run validation when value changes
	$effect(() => {
		if (!validation) return;

		// Clear any existing show timer when value changes
		if (showTimer) {
			clearTimeout(showTimer);
			shouldShow = false;
		}

		// Clear previous validation timer
		if (debounceTimer) {
			clearTimeout(debounceTimer);
		}

		// Run validation immediately to get result, but don't show yet
		const result = validation.validate(value, context);
		validationResult = result;

		// If force validate, show immediately
		if (forceValidate) {
			shouldShow = true;
		}

		// Set timer to show validation after debounce
		if (validation.showOn === 'change' || !validation.showOn) {
			showTimer = setTimeout(() => {
				shouldShow = true;
			}, validation.debounceMs || 500);
		}

		return () => {
			if (debounceTimer) {
				clearTimeout(debounceTimer);
			}
			if (showTimer) {
				clearTimeout(showTimer);
			}
		};
	});

	function handleBlur(e: FocusEvent & { currentTarget: HTMLInputElement }) {
		// Show validation on blur if we have a result
		if (validation && validationResult && (validation.showOn === 'blur' || validation.showOn === 'change' || !validation.showOn)) {
			shouldShow = true;
		}
		onBlur?.(e);
	}

	// Determine validation state class
	const validationClass = $derived(() => {
		if (!shouldShow || !showValidation || !validationResult) {
			return '';
		}

		if (validationResult.severity === 'warning') {
			return 'input-warning';
		}

		if (!validationResult.isValid) {
			return 'input-error';
		}

		return 'input-success';
	});

	// Get the hint text to display
	const hintText = $derived(() => {
		if (!shouldShow || !showValidation || !validationResult) return null;
		return validationResult.hint || validationResult.warning;
	});

	// Get hint class
	const hintClass = $derived(() => {
		if (!validationResult) return '';

		if (validationResult.severity === 'warning') {
			return 'text-warning';
		}

		if (!validationResult.isValid) {
			return 'text-error';
		}

		return 'text-base-content/60';
	});

	const className = $derived(`input input-bordered w-full ${sizeClasses[size]} ${validationClass()} ${extraClass}`.trim());
</script>

<div class="w-full">
	<input
		{type}
		{placeholder}
		{disabled}
		bind:value
		onblur={handleBlur}
		class={className}
	/>

	<!-- Validation hint -->
	{#if hintText()}
		<div
			class="label py-1"
			role="status"
			aria-live="polite"
		>
			<span class="label-text-alt {hintClass()}">{hintText()}</span>
		</div>
	{/if}
</div>
