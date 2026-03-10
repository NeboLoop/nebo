<!--
  Collapsible Component
  Simple collapsible container using DaisyUI collapse
-->

<script lang="ts">
	interface Props {
		open?: boolean;
		onOpenChange?: (open: boolean) => void;
		trigger?: any;
		children: any;
	}

	let {
		open = $bindable(false),
		onOpenChange,
		trigger,
		children
	}: Props = $props();

	function toggleOpen() {
		open = !open;
		onOpenChange?.(open);
	}
</script>

<div class="collapse collapse-arrow bg-base-200 {open ? 'collapse-open' : 'collapse-close'}">
	{#if trigger}
		<div
			class="collapse-title text-lg font-medium cursor-pointer"
			onclick={toggleOpen}
			role="button"
			tabindex="0"
			onkeydown={(e) => e.key === 'Enter' && toggleOpen()}
		>
			{@render trigger()}
		</div>
	{/if}

	<div class="collapse-content">
		{@render children()}
	</div>
</div>
