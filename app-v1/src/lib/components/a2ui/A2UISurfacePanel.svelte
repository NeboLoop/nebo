<!--
  A2UISurfacePanel — renders an A2UI surface for a given surfaceId.

  Wraps the <a2ui-surface> Lit web component. The surface model
  comes from the a2ui store's MessageProcessor, which tracks all
  active surfaces and their component/data state.

  NOTE: The <a2ui-surface> custom element is registered globally
  when @a2ui/lit is imported. Svelte treats unknown elements as
  web components automatically.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { a2ui } from '$lib/stores/a2ui';
	import { getWebSocketClient } from '$lib/websocket/client';
	import type { SurfaceModel } from '@a2ui/web_core/v0_9';
	import type { LitComponentApi } from '@a2ui/lit/v0_9';
	import type { NeboSurfaceElement } from './nebo-surface';
	// Side-effect import: registers <nebo-a2ui-surface> (shadow DOM + style injection)
	import './nebo-surface';
	import '@a2ui/lit/v0_9';
	// Side-effect import: registers <a2ui-markdown-provider> element
	import './a2ui-markdown-provider';

	// Note: nebo-a2ui-* custom elements are registered when the store's
	// init() dynamically imports the catalog (client-only, avoids SSR).

	let { surfaceId, onClose }: { surfaceId: string; onClose?: () => void } = $props();

	let surfaceModel: SurfaceModel<LitComponentApi> | undefined = $state(undefined);
	let surfaceElement: HTMLElement | undefined = $state(undefined);
	let containerElement: HTMLElement | undefined = $state(undefined);

	// Subscribe to surface creation for our surfaceId
	onMount(() => {
		const processor = a2ui.getProcessor();
		if (!processor) return;

		// Check if surface already exists
		const existing = processor.model.getSurface(surfaceId);
		if (existing) {
			surfaceModel = existing;
		}

		// Listen for new surface creation
		const unsub = processor.onSurfaceCreated((surface) => {
			if (surface.id === surfaceId) {
				surfaceModel = surface;
			}
		});

		return () => {
			unsub.unsubscribe();
		};
	});

	// Update the web component's surface property when model changes
	$effect(() => {
		if (surfaceElement && surfaceModel) {
			(surfaceElement as any).surface = surfaceModel;
		}
	});

	// Subscribe to action events from the surface and forward to backend via WebSocket.
	// Skip if the action is already being processed (prevents double-click duplicates).
	$effect(() => {
		if (!surfaceModel) return;
		const sub = surfaceModel.onAction.subscribe((action: any) => {
			if (a2ui.isActionPending(surfaceId, action.name)) {
				console.log('[a2ui] action already pending, skipping:', action.name);
				return;
			}
			console.log('[a2ui] action dispatched:', action.name, 'from', action.sourceComponentId);
			getWebSocketClient().send('a2ui_action', {
				surfaceId: surfaceId,
				name: action.name,
				sourceComponentId: action.sourceComponentId,
				context: action.context ?? {},
			});
		});
		return () => sub.unsubscribe();
	});

	// Notify the surface element's Lit context when actions complete,
	// so NeboButton can clear its loading spinner.
	$effect(() => {
		const unsub = a2ui.subscribe((state) => {
			if (!surfaceElement) return;
			// If no actions pending for this surface, notify completion
			const hasPending = [...state.pendingActions].some((key) => key.startsWith(surfaceId + ':'));
			if (!hasPending) {
				(surfaceElement as NeboSurfaceElement).notifyActionComplete();
			}
		});
		return unsub;
	});

</script>

<div class="a2ui-surface-container" data-surface-id={surfaceId} bind:this={containerElement}>
	{#if surfaceModel}
		<a2ui-markdown-provider>
			<nebo-a2ui-surface bind:this={surfaceElement}>
				<div slot="loading">Loading workspace...</div>
			</nebo-a2ui-surface>
		</a2ui-markdown-provider>
	{:else}
		<div class="a2ui-loading">
			<span class="loading loading-spinner loading-md"></span>
		</div>
	{/if}
</div>
