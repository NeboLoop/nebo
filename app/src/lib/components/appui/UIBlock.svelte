<!--
  UIBlock - Renders a single block from an app's structured template UI.
  Handles all 8 block types: text, heading, input, button, select, toggle, divider, image.
-->

<script lang="ts">
	import type { UIBlock } from '$lib/api/nebo';

	interface Props {
		block: UIBlock;
		onEvent: (blockId: string, action: string, value: string) => void;
	}

	let { block, onEvent }: Props = $props();

	let inputValue = $state('');
	let toggleChecked = $state(false);

	// Sync local state when the block prop value changes (e.g., server returns updated view)
	$effect(() => {
		inputValue = block.value ?? '';
		toggleChecked = block.value === 'true';
	});

	// Map button variants to DaisyUI btn classes
	const buttonVariantMap: Record<string, string> = {
		primary: 'btn-primary',
		secondary: 'btn-secondary',
		ghost: 'btn-ghost',
		error: 'btn-error'
	};

	const buttonClass = $derived(
		`btn ${buttonVariantMap[block.variant ?? ''] ?? 'btn-neutral'}`
	);

	function handleInputChange(e: Event) {
		const target = e.currentTarget as HTMLInputElement;
		inputValue = target.value;
	}

	function handleInputBlur() {
		onEvent(block.block_id, 'input_change', inputValue);
	}

	function handleInputKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			onEvent(block.block_id, 'input_change', inputValue);
		}
	}

	function handleButtonClick() {
		onEvent(block.block_id, 'click', block.value ?? '');
	}

	function handleSelectChange(e: Event) {
		const target = e.currentTarget as HTMLSelectElement;
		onEvent(block.block_id, 'select_change', target.value);
	}

	function handleToggleChange(e: Event) {
		const target = e.currentTarget as HTMLInputElement;
		toggleChecked = target.checked;
		onEvent(block.block_id, 'toggle_change', String(toggleChecked));
	}
</script>

{#if block.type === 'text'}
	<p class="text-base-content">{block.text}</p>

{:else if block.type === 'heading'}
	{#if block.variant === 'h1'}
		<h1 class="text-2xl font-bold text-base-content">{block.text}</h1>
	{:else if block.variant === 'h3'}
		<h3 class="text-lg font-semibold text-base-content">{block.text}</h3>
	{:else}
		<h2 class="text-xl font-bold text-base-content">{block.text}</h2>
	{/if}

{:else if block.type === 'input'}
	<div class="form-control w-full">
		{#if block.text}
			<label class="label" for={block.block_id}>
				<span class="label-text">{block.text}</span>
			</label>
		{/if}
		<input
			id={block.block_id}
			type="text"
			class="input input-bordered w-full"
			placeholder={block.placeholder ?? ''}
			value={inputValue}
			disabled={block.disabled ?? false}
			oninput={handleInputChange}
			onblur={handleInputBlur}
			onkeydown={handleInputKeydown}
		/>
		{#if block.hint}
			<label class="label" for={block.block_id}>
				<span class="label-text-alt text-base-content/50">{block.hint}</span>
			</label>
		{/if}
	</div>

{:else if block.type === 'button'}
	<button
		type="button"
		class={buttonClass}
		disabled={block.disabled ?? false}
		onclick={handleButtonClick}
	>
		{block.text}
	</button>

{:else if block.type === 'select'}
	<div class="form-control w-full">
		{#if block.text}
			<label class="label" for={block.block_id}>
				<span class="label-text">{block.text}</span>
			</label>
		{/if}
		<select
			id={block.block_id}
			class="select select-bordered w-full"
			disabled={block.disabled ?? false}
			value={block.value ?? ''}
			onchange={handleSelectChange}
		>
			{#if block.placeholder}
				<option value="" disabled>{block.placeholder}</option>
			{/if}
			{#each block.options ?? [] as opt}
				<option value={opt.value}>{opt.label}</option>
			{/each}
		</select>
		{#if block.hint}
			<label class="label" for={block.block_id}>
				<span class="label-text-alt text-base-content/50">{block.hint}</span>
			</label>
		{/if}
	</div>

{:else if block.type === 'toggle'}
	<div class="form-control">
		<label class="label cursor-pointer justify-start gap-3" for={block.block_id}>
			<input
				id={block.block_id}
				type="checkbox"
				class="toggle toggle-primary"
				checked={toggleChecked}
				disabled={block.disabled ?? false}
				onchange={handleToggleChange}
			/>
			{#if block.text}
				<span class="label-text">{block.text}</span>
			{/if}
		</label>
	</div>

{:else if block.type === 'divider'}
	<div class="divider"></div>

{:else if block.type === 'image'}
	<img
		src={block.src}
		alt={block.alt ?? ''}
		class="rounded-lg max-w-full"
	/>
{/if}
