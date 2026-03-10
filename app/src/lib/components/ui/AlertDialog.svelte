<!--
  AlertDialog Component
  Alert/confirmation dialog using DaisyUI modal
-->

<script lang="ts">
	import { Button } from '$lib/components/ui';
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
		onclose?: () => void; // alias for onCancel
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
		onclose, // alias for onCancel
		onOpenChange
	}: Props = $props();

	let dialogRef: HTMLDialogElement | undefined = $state();

	$effect(() => {
		if (dialogRef) {
			if (open) {
				dialogRef.showModal();
			} else {
				dialogRef.close();
			}
		}
	});

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

	function handleDialogClose() {
		open = false;
		onOpenChange?.(false);
	}
</script>

<dialog
	bind:this={dialogRef}
	class="modal"
	onclose={handleDialogClose}
>
	<div class="modal-box">
		<div class="flex items-start justify-between mb-4">
			{#if title}
				<h3 class="text-lg font-bold">{title}</h3>
			{/if}
			<button
				onclick={close}
				class="btn btn-sm btn-circle btn-ghost"
				aria-label="Close"
			>
				<X class="h-5 w-5" />
			</button>
		</div>

		{#if description}
			<p class="text-base-content/70 mb-4">{description}</p>
		{/if}
		{#if children}
			{@render children()}
		{/if}

		<div class="modal-action">
			<Button type="secondary" onclick={handleCancel}>
				{cancelLabel}
			</Button>
			<Button type={actionType} onclick={handleAction}>
				{actionLabel}
			</Button>
		</div>
	</div>
	<form method="dialog" class="modal-backdrop">
		<button onclick={close}>close</button>
	</form>
</dialog>
