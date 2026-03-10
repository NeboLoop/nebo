<!--
  Drawer/Slide-over Component
  Sliding panel from edge of screen
-->

<script lang="ts">
	import { X } from 'lucide-svelte';

	interface Props {
		open?: boolean;
		title?: string;
		position?: 'left' | 'right';
		size?: 'sm' | 'md' | 'lg' | 'xl' | 'full';
		onclose?: () => void;
		children: any;
	}

	let {
		open = $bindable(false),
		title,
		position = 'right',
		size = 'md',
		onclose,
		children
	}: Props = $props();

	// Generate unique ID for the drawer toggle
	const drawerId = `drawer-${Math.random().toString(36).substr(2, 9)}`;

	function handleClose() {
		open = false;
		if (onclose) {
			onclose();
		}
	}

	const sizeClasses = {
		sm: 'w-80',
		md: 'w-96',
		lg: 'w-[32rem]',
		xl: 'w-[40rem]',
		full: 'w-full'
	};
</script>

<div class="drawer {position === 'right' ? 'drawer-end' : ''} z-50">
	<input id={drawerId} type="checkbox" class="drawer-toggle" bind:checked={open} />

	<div class="drawer-side">
		<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
		<label for={drawerId} aria-label="close sidebar" class="drawer-overlay" onclick={handleClose}></label>
		<div class="menu bg-base-200 text-base-content min-h-full {sizeClasses[size]} p-0">
			<!-- Header -->
			{#if title}
				<div class="flex items-center justify-between px-6 py-4 border-b border-base-300">
					<h2 class="text-xl font-semibold">{title}</h2>
					<button
						type="button"
						onclick={handleClose}
						class="btn btn-ghost btn-sm btn-circle"
						aria-label="Close drawer"
					>
						<X class="h-5 w-5" />
					</button>
				</div>
			{/if}

			<!-- Content -->
			<div class="flex-1 overflow-y-auto p-6">
				{@render children()}
			</div>
		</div>
	</div>
</div>
