<!--
  AlertDialog Component
  Matches NeboLoop.com modal style
-->

<script lang="ts">
	import { X } from 'lucide-svelte';

	interface Props {
		open?: boolean;
		title?: string;
		description?: string;
		actionLabel?: string;
		cancelLabel?: string;
		actionType?: 'primary' | 'danger';
		children?: any;
		onAction?: () => void;
		onCancel?: () => void;
		onclose?: () => void;
		onOpenChange?: (open: boolean) => void;
	}

	let {
		open = $bindable(false),
		title = '',
		description = '',
		actionLabel = 'Continue',
		cancelLabel = 'Cancel',
		actionType = 'primary',
		children,
		onAction,
		onCancel,
		onclose,
		onOpenChange
	}: Props = $props();

	function handleAction() {
		onAction?.();
		close();
	}

	function handleCancel() {
		(onCancel || onclose)?.();
		close();
	}

	function close() {
		open = false;
		onOpenChange?.(false);
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			handleCancel();
		}
	}
</script>

{#if open}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="nebo-modal-backdrop"
		onkeydown={handleKeydown}
		role="alertdialog"
		aria-modal="true"
		tabindex="-1"
	>
		<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
		<button type="button" class="nebo-modal-overlay" onclick={handleCancel}></button>

		<div class="nebo-modal-card max-w-md">
			<!-- Header -->
			<div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
				{#if title}
					<h3 class="font-display text-lg font-bold">{title}</h3>
				{/if}
				<button
					type="button"
					onclick={close}
					class="nebo-modal-close"
					aria-label="Close"
				>
					<X class="w-5 h-5 text-base-content/90" />
				</button>
			</div>

			<!-- Body -->
			<div class="px-5 py-5">
				{#if description}
					<p class="text-sm text-base-content/70 mb-4">{description}</p>
				{/if}
				{#if children}
					{@render children()}
				{/if}
			</div>

			<!-- Footer -->
			<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-base-content/10">
				<button
					type="button"
					class="h-10 px-5 rounded-full border border-base-content/10 text-sm font-medium hover:bg-base-content/5 transition-colors"
					onclick={handleCancel}
				>
					{cancelLabel}
				</button>
				<button
					type="button"
					class="h-10 px-6 rounded-full text-sm font-bold transition-all {actionType === 'danger' ? 'bg-error text-white hover:brightness-110' : 'bg-primary text-primary-content hover:brightness-110'}"
					onclick={handleAction}
				>
					{actionLabel}
				</button>
			</div>
		</div>
	</div>
{/if}
