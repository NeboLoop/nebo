<!--
  Sheet Component
  Slide-in panel from the side (similar to Drawer but with different API)
-->

<script lang="ts">
	interface Props {
		open?: boolean;
		side?: 'top' | 'bottom' | 'left' | 'right';
		title?: string;
		description?: string;
		children?: any;
		onOpenChange?: (open: boolean) => void;
	}

	let {
		open = $bindable(false),
		side = 'right',
		title = '',
		description = '',
		children,
		onOpenChange
	}: Props = $props();

	// Generate unique ID for the drawer toggle
	const sheetId = `sheet-${Math.random().toString(36).substr(2, 9)}`;

	function close() {
		open = false;
		onOpenChange?.(false);
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			close();
		}
	}

	const sideClasses = {
		top: 'w-full',
		bottom: 'w-full',
		left: 'w-80 sm:w-96 h-full',
		right: 'w-80 sm:w-96 h-full'
	};

	const isHorizontal = $derived(side === 'left' || side === 'right');
	const drawerClass = $derived(side === 'right' ? 'drawer-end' : '');
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="drawer {drawerClass} z-50">
	<input id={sheetId} type="checkbox" class="drawer-toggle" bind:checked={open} />

	<div class="drawer-side">
		<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
		<label for={sheetId} aria-label="close sidebar" class="drawer-overlay" onclick={close}></label>
		<div class="bg-base-100 text-base-content min-h-full {sideClasses[side]} p-6 relative">
			<!-- Close button -->
			<button
				type="button"
				onclick={close}
				class="btn btn-ghost btn-sm btn-circle absolute right-4 top-4"
			>
				<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
				</svg>
				<span class="sr-only">Close</span>
			</button>

			<!-- Header -->
			{#if title || description}
				<div class="flex flex-col space-y-2 text-center sm:text-left mb-4">
					{#if title}
						<h2 class="text-lg font-semibold">{title}</h2>
					{/if}
					{#if description}
						<p class="text-sm text-base-content/70">{description}</p>
					{/if}
				</div>
			{/if}

			<!-- Content -->
			{#if children}
				{@render children()}
			{/if}
		</div>
	</div>
</div>
