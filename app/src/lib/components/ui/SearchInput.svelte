<!--
  Search Input Component
  Input with search icon and clear button
-->

<script lang="ts">
	interface Props {
		value?: string;
		placeholder?: string;
		disabled?: boolean;
		loading?: boolean;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		onsearch?: (value: string) => void;
		onclear?: () => void;
		onkeydown?: (e: KeyboardEvent) => void;
		class?: string;
	}

	let {
		value = $bindable(''),
		placeholder = 'Search...',
		disabled = false,
		loading = false,
		size = 'md',
		onsearch,
		onclear,
		onkeydown,
		class: extraClass = ''
	}: Props = $props();

	const sizeClasses: Record<string, string> = {
		xs: 'input-xs',
		sm: 'input-sm',
		md: 'input-md',
		lg: 'input-lg'
	};

	function handleInput(event: Event) {
		const target = event.target as HTMLInputElement;
		value = target.value;
		onsearch?.(value);
	}

	function handleClear() {
		value = '';
		onclear?.();
	}
</script>

<div class="relative {extraClass}">
	<div class="pointer-events-none absolute inset-y-0 left-0 flex items-center pl-3">
		{#if loading}
			<span class="loading loading-spinner loading-sm"></span>
		{:else}
			<svg class="h-4 w-4 text-base-content/40" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
			</svg>
		{/if}
	</div>
	<input
		type="text"
		bind:value
		{placeholder}
		{disabled}
		oninput={handleInput}
		{onkeydown}
		class="input input-bordered w-full pl-10 {value ? 'pr-10' : ''} {sizeClasses[size]}"
	/>
	{#if value && !disabled}
		<button
			type="button"
			onclick={handleClear}
			class="btn btn-ghost btn-sm btn-circle absolute inset-y-0 right-1 my-auto"
			aria-label="Clear search"
		>
			<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
			</svg>
		</button>
	{/if}
</div>
