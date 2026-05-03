<!--
  Button Group Component using DaisyUI join
  Segmented control buttons with shared borders
-->

<script lang="ts">
	interface Button {
		value: string;
		label: string;
		icon?: string;
	}

	interface Props {
		buttons: Button[];
		selected?: string;
		onselect?: (value: string) => void;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		class?: string;
	}

	let {
		buttons,
		selected = $bindable(''),
		onselect,
		size = 'md',
		class: extraClass = ''
	}: Props = $props();

	const sizeClasses: Record<string, string> = {
		xs: 'btn-xs',
		sm: 'btn-sm',
		md: '',
		lg: 'btn-lg'
	};

	function handleSelect(value: string) {
		selected = value;
		if (onselect) {
			onselect(value);
		}
	}
</script>

<div class="join {extraClass}">
	{#each buttons as button}
		<button
			type="button"
			onclick={() => handleSelect(button.value)}
			class="btn join-item {sizeClasses[size]} {selected === button.value ? 'btn-primary' : 'btn-ghost'}"
		>
			{#if button.icon}
				<i class="fas fa-{button.icon}"></i>
			{/if}
			{button.label}
		</button>
	{/each}
</div>
