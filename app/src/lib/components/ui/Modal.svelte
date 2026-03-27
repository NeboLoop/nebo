<!--
  Modal Component
  Matches NeboLoop.com modal style
-->

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { X } from 'lucide-svelte';

	interface Props {
		show?: boolean;
		open?: boolean;
		title: string;
		description?: string;
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
		description = '',
		size = 'md',
		closeOnBackdrop = true,
		closeOnEscape = true,
		showCloseButton = true,
		onclose,
		children,
		footer
	}: Props = $props();

	let isOpen = $derived(show || open);

	const widthClasses: Record<string, string> = {
		sm: 'max-w-md',
		md: 'max-w-lg',
		lg: 'max-w-2xl',
		xl: 'max-w-4xl',
		full: 'max-w-5xl'
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

{#if isOpen}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="nebo-modal-backdrop"
		onkeydown={handleKeydown}
		role="dialog"
		aria-modal="true"
		tabindex="-1"
	>
		<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
		<button type="button" class="nebo-modal-overlay" onclick={handleBackdropClick}></button>

		<div class="nebo-modal-card {widthClasses[size]} {size === 'full' ? 'nebo-modal-full' : ''}">
			<!-- Header -->
			<div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
				<div>
					<h3 class="font-display text-lg font-bold">{title}</h3>
					{#if description}
						<p class="text-base text-base-content/90 mt-0.5">{description}</p>
					{/if}
				</div>
				{#if showCloseButton}
					<button
						type="button"
						onclick={closeModal}
						class="nebo-modal-close"
						aria-label={$t('common.close')}
					>
						<X class="w-5 h-5 text-base-content/90" />
					</button>
				{/if}
			</div>

			<!-- Body -->
			<div class="px-5 py-5 overflow-y-auto {size === 'full' ? 'max-h-[calc(100vh-10rem)]' : 'max-h-[60vh]'}">
				{@render children()}
			</div>

			<!-- Footer -->
			{#if footer}
				<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-base-content/10">
					{@render footer()}
				</div>
			{/if}
		</div>
	</div>
{/if}
