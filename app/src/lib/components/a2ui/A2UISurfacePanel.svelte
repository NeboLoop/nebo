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
	import { onMount, onDestroy } from 'svelte';
	import { a2ui } from '$lib/stores/a2ui';
	import type { SurfaceModel } from '@a2ui/web_core/v0_9';
	import type { LitComponentApi } from '@a2ui/lit/v0_9';
	// Side-effect import: registers <a2ui-surface> custom element
	import '@a2ui/lit/v0_9';

	let { surfaceId }: { surfaceId: string } = $props();

	let surfaceModel: SurfaceModel<LitComponentApi> | undefined = $state(undefined);
	let surfaceElement: HTMLElement | undefined = $state(undefined);

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
</script>

<div class="a2ui-surface-container" data-surface-id={surfaceId}>
	{#if surfaceModel}
		<a2ui-surface bind:this={surfaceElement}></a2ui-surface>
	{:else}
		<div class="a2ui-loading">
			<span class="loading loading-spinner loading-md"></span>
		</div>
	{/if}
</div>
