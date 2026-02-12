<!--
  Modal Component
  Using DaisyUI modal classes
-->

<script lang="ts">
	import { X } from 'lucide-svelte';

	interface Props {
		show?: boolean;
		open?: boolean;
		title: string;
		size?: 'sm' | 'md' | 'lg' | 'xl' | 'full';
		closeOnBackdrop?: boolean;
		closeOnEscape?: boolean;
		showCloseButton?: boolean;
		onclose?: () => void;
		children: any;
		footer?: any;
	}

	let {
		show = $bindable(false),
		open = $bindable(false),
		title,
		size = 'md',
		closeOnBackdrop = false,
		closeOnEscape = true,
		showCloseButton = true,
		onclose,
		children,
		footer
	}: Props = $props();

	// Support both 'show' and 'open' props
	let isOpen = $derived(show || open);

	const sizeClasses: Record<string, string> = {
		sm: 'modal-box max-w-md',
		md: 'modal-box max-w-lg',
		lg: 'modal-box max-w-2xl',
		xl: 'modal-box max-w-4xl',
		full: 'modal-box w-full max-w-5xl h-[calc(100vh-2rem)] max-h-[calc(100vh-2rem)] overflow-x-hidden'
	};

	function closeModal() {
		show = false;
		open = false;
		onclose?.();
	}

	function handleBackdropClick() {
		if (closeOnBackdrop) {
			closeModal();
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && closeOnEscape) {
			closeModal();
		}
	}
</script>

<div class="modal" class:modal-open={isOpen} onkeydown={handleKeydown} role="dialog" aria-modal="true" tabindex="-1">
	<div class={sizeClasses[size]}>
		<!-- Header -->
		<div class="flex items-center justify-between pb-4 border-b border-base-300">
			<h3 class="text-xl font-bold">{title}</h3>
			{#if showCloseButton}
				<button
					onclick={closeModal}
					class="btn btn-sm btn-circle btn-ghost"
					aria-label="Close"
				>
					<X class="h-5 w-5" />
				</button>
			{/if}
		</div>

		<!-- Body -->
		<div class="py-4 overflow-y-auto {size === 'full' ? 'max-h-[calc(100vh-10rem)]' : 'max-h-[60vh]'}">
			{@render children()}
		</div>

		<!-- Footer (if provided) -->
		{#if footer}
			<div class="modal-action pt-4 border-t border-base-300">
				{@render footer()}
			</div>
		{/if}
	</div>
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div class="modal-backdrop" onclick={handleBackdropClick}>
		<button class="cursor-default">close</button>
	</div>
</div>
