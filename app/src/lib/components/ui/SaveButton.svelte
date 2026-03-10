<!--
  SaveButton Component
  Button with saving/saved state feedback
  - Shows "Saving..." with spinner when loading
  - Shows "Saved" briefly on success
  - Shows original text otherwise
-->

<script lang="ts">
	import Button from './Button.svelte';
	import Spinner from './Spinner.svelte';

	let {
		saving = false,
		saved = false,
		label = 'Save',
		savingLabel = 'Saving...',
		savedLabel = 'Saved',
		type = 'primary',
		size = 'md',
		disabled = false,
		onclick,
		htmlType = 'button',
		class: extraClass = ''
	}: {
		saving?: boolean;
		saved?: boolean;
		label?: string;
		savingLabel?: string;
		savedLabel?: string;
		type?: 'primary' | 'secondary' | 'link' | 'danger' | 'ghost';
		size?: 'sm' | 'md' | 'lg' | 'icon';
		disabled?: boolean;
		onclick?: () => void | Promise<void>;
		htmlType?: 'button' | 'submit' | 'reset';
		class?: string;
	} = $props();

	let displayState = $derived(
		saving ? 'saving' : saved ? 'saved' : 'idle'
	);

	function handleClick() {
		if (onclick) {
			onclick();
		}
	}
</script>

<Button
	{type}
	{size}
	disabled={disabled || saving}
	onclick={handleClick}
	{htmlType}
	class={extraClass}
>
	{#if displayState === 'saving'}
		<span class="inline-flex items-center gap-2">
			<Spinner size={14} />
			{savingLabel}
		</span>
	{:else if displayState === 'saved'}
		<span class="inline-flex items-center gap-1.5">
			<svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
				<path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
			</svg>
			{savedLabel}
		</span>
	{:else}
		{label}
	{/if}
</Button>
