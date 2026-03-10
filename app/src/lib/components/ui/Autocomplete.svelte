<!--
  Autocomplete Component
  Input with suggestions dropdown
-->

<script lang="ts">
	import { Search, X, Loader2 } from 'lucide-svelte';

	interface Option {
		value: string;
		label: string;
		description?: string;
		icon?: any;
		disabled?: boolean;
	}

	interface Props {
		value?: string;
		options?: Option[];
		placeholder?: string;
		label?: string;
		size?: 'sm' | 'md' | 'lg';
		disabled?: boolean;
		loading?: boolean;
		clearable?: boolean;
		showIcon?: boolean;
		minChars?: number;
		maxResults?: number;
		emptyMessage?: string;
		class?: string;
		onselect?: (option: Option) => void;
		onsearch?: (query: string) => void;
		onchange?: (value: string) => void;
	}

	let {
		value = $bindable(''),
		options = [],
		placeholder = 'Search...',
		label,
		size = 'md',
		disabled = false,
		loading = false,
		clearable = true,
		showIcon = true,
		minChars = 1,
		maxResults = 10,
		emptyMessage = 'No results found',
		class: extraClass = '',
		onselect,
		onsearch,
		onchange
	}: Props = $props();

	let isOpen = $state(false);
	let highlightedIndex = $state(-1);
	let inputRef = $state<HTMLInputElement | null>(null);

	// Filter options based on search value
	const filteredOptions = $derived(() => {
		if (!value || value.length < minChars) return [];
		const query = value.toLowerCase();
		return options
			.filter(
				(opt) =>
					opt.label.toLowerCase().includes(query) ||
					opt.description?.toLowerCase().includes(query)
			)
			.slice(0, maxResults);
	});

	const showDropdown = $derived(isOpen && (filteredOptions().length > 0 || (value.length >= minChars && !loading)));

	const sizeClasses: Record<string, string> = {
		sm: 'input-sm',
		md: 'input-md',
		lg: 'input-lg'
	};

	function handleInput(e: Event) {
		const target = e.currentTarget as HTMLInputElement;
		value = target.value;
		isOpen = true;
		highlightedIndex = -1;
		onsearch?.(value);
		onchange?.(value);
	}

	function handleFocus() {
		if (value.length >= minChars) {
			isOpen = true;
		}
	}

	function handleBlur() {
		// Delay to allow click on option
		setTimeout(() => {
			isOpen = false;
			highlightedIndex = -1;
		}, 200);
	}

	function handleKeydown(e: KeyboardEvent) {
		const opts = filteredOptions();

		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				highlightedIndex = Math.min(highlightedIndex + 1, opts.length - 1);
				break;
			case 'ArrowUp':
				e.preventDefault();
				highlightedIndex = Math.max(highlightedIndex - 1, -1);
				break;
			case 'Enter':
				e.preventDefault();
				if (highlightedIndex >= 0 && opts[highlightedIndex]) {
					selectOption(opts[highlightedIndex]);
				}
				break;
			case 'Escape':
				isOpen = false;
				highlightedIndex = -1;
				break;
		}
	}

	function selectOption(option: Option) {
		if (option.disabled) return;
		value = option.label;
		isOpen = false;
		highlightedIndex = -1;
		onselect?.(option);
		onchange?.(option.value);
	}

	function clear() {
		value = '';
		isOpen = false;
		highlightedIndex = -1;
		inputRef?.focus();
		onchange?.('');
	}
</script>

<div class="relative w-full {extraClass}">
	{#if label}
		<label class="label">
			<span class="label-text font-medium">{label}</span>
		</label>
	{/if}

	<div class="relative">
		{#if showIcon}
			<div class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
				{#if loading}
					<Loader2 class="w-4 h-4 text-base-content/40 animate-spin" />
				{:else}
					<Search class="w-4 h-4 text-base-content/40" />
				{/if}
			</div>
		{/if}

		<input
			bind:this={inputRef}
			type="text"
			class="input input-bordered w-full {sizeClasses[size]}"
			class:pl-10={showIcon}
			class:pr-10={clearable && value}
			{placeholder}
			{disabled}
			{value}
			oninput={handleInput}
			onfocus={handleFocus}
			onblur={handleBlur}
			onkeydown={handleKeydown}
			role="combobox"
			aria-expanded={showDropdown}
			aria-haspopup="listbox"
			aria-autocomplete="list"
		/>

		{#if clearable && value && !disabled}
			<button
				type="button"
				class="absolute inset-y-0 right-0 pr-3 flex items-center"
				onclick={clear}
				tabindex="-1"
			>
				<X class="w-4 h-4 text-base-content/40 hover:text-base-content/60" />
			</button>
		{/if}
	</div>

	{#if showDropdown}
		<ul
			class="absolute z-50 w-full mt-1 bg-base-100 border border-base-300 rounded-lg shadow-lg max-h-60 overflow-auto"
			role="listbox"
		>
			{#if filteredOptions().length === 0}
				<li class="px-4 py-3 text-sm text-base-content/60">
					{emptyMessage}
				</li>
			{:else}
				{#each filteredOptions() as option, index}
					<li
						role="option"
						aria-selected={index === highlightedIndex}
						class="px-4 py-2 cursor-pointer transition-colors"
						class:bg-base-200={index === highlightedIndex}
						class:opacity-50={option.disabled}
						class:cursor-not-allowed={option.disabled}
						onmouseenter={() => (highlightedIndex = index)}
						onclick={() => selectOption(option)}
					>
						<div class="flex items-center gap-3">
							{#if option.icon}
								<svelte:component this={option.icon} class="w-4 h-4 text-base-content/60" />
							{/if}
							<div class="flex-1 min-w-0">
								<div class="font-medium truncate">{option.label}</div>
								{#if option.description}
									<div class="text-sm text-base-content/60 truncate">
										{option.description}
									</div>
								{/if}
							</div>
						</div>
					</li>
				{/each}
			{/if}
		</ul>
	{/if}
</div>
