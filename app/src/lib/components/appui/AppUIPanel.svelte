<!--
  AppUIPanel - Fixed right panel that renders an app's structured template UI.
  Follows the same pattern as ToolOutputSidebar.svelte.
-->

<script lang="ts">
	import { X, AlertCircle, Loader2 } from 'lucide-svelte';
	import UIBlock from './UIBlock.svelte';
	import Toast from '$lib/components/ui/Toast.svelte';
	import { getUIView, sendUIEvent } from '$lib/api/nebo';
	import type { UIView } from '$lib/api/nebo';

	interface Props {
		appId: string;
		appName: string;
		onClose: () => void;
	}

	let { appId, appName, onClose }: Props = $props();

	let view: UIView | null = $state(null);
	let loading = $state(true);
	let error = $state('');
	let sending = $state(false);
	let toastMessage = $state('');
	let toastShow = $state(false);
	let toastType: 'success' | 'error' | 'warning' | 'info' = $state('info');

	// Load the view when the component mounts or appId changes
	$effect(() => {
		if (appId) {
			loadView();
		}
	});

	async function loadView() {
		loading = true;
		error = '';
		try {
			view = await getUIView(appId);
		} catch (err: any) {
			error = err.message || 'Failed to load app view';
			view = null;
		} finally {
			loading = false;
		}
	}

	async function handleEvent(blockId: string, action: string, value: string) {
		if (!view || sending) return;
		sending = true;
		try {
			const resp = await sendUIEvent({
				view_id: view.view_id,
				block_id: blockId,
				action,
				value
			}, appId);

			if (resp.error) {
				toastType = 'error';
				toastMessage = resp.error;
				toastShow = true;
			}

			if (resp.toast) {
				toastType = 'success';
				toastMessage = resp.toast;
				toastShow = true;
			}

			// If the server returned an updated view, replace the current one
			if (resp.view) {
				view = resp.view;
			}
		} catch (err: any) {
			toastType = 'error';
			toastMessage = err.message || 'Failed to send event';
			toastShow = true;
		} finally {
			sending = false;
		}
	}
</script>

<!-- Sidebar panel -->
<div
	class="fixed inset-y-0 right-0 w-full sm:w-[480px] bg-base-100 border-l border-base-300 shadow-2xl z-50 flex flex-col animate-slide-in-right"
>
	<!-- Header -->
	<div class="flex items-center justify-between px-5 py-4 border-b border-base-300">
		<h2 class="text-lg font-semibold text-base-content truncate">{view?.title ?? appName}</h2>
		<button
			type="button"
			onclick={onClose}
			class="p-1.5 rounded-lg hover:bg-base-200 text-base-content/60 hover:text-base-content transition-colors"
			title="Close"
		>
			<X class="w-5 h-5" />
		</button>
	</div>

	<!-- Content -->
	<div class="flex-1 overflow-y-auto p-5">
		{#if loading}
			<div class="flex flex-col items-center justify-center h-full gap-3">
				<Loader2 class="w-8 h-8 text-base-content/40 animate-spin" />
				<p class="text-sm text-base-content/50">Loading app...</p>
			</div>
		{:else if error}
			<div class="flex flex-col items-center justify-center h-full gap-3">
				<AlertCircle class="w-8 h-8 text-error" />
				<p class="text-sm text-error">{error}</p>
				<button type="button" class="btn btn-sm btn-ghost" onclick={loadView}>
					Retry
				</button>
			</div>
		{:else if view}
			<div class="flex flex-col gap-4">
				{#each view.blocks as block (block.block_id)}
					<UIBlock {block} onEvent={handleEvent} />
				{/each}
			</div>
		{:else}
			<p class="text-sm text-base-content/50 text-center">No view available.</p>
		{/if}
	</div>
</div>

<!-- Backdrop for mobile -->
<button
	type="button"
	class="fixed inset-0 bg-black/50 z-40 sm:bg-black/20"
	onclick={onClose}
	aria-label="Close app panel"
></button>

<!-- Toast notifications -->
<Toast message={toastMessage} type={toastType} bind:show={toastShow} />
