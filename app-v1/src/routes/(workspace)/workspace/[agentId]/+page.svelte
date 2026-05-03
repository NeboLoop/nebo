<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { get } from 'svelte/store';
	import { page } from '$app/state';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { a2ui, surfacesForAgent } from '$lib/stores/a2ui';
	import { auth } from '$lib/stores/auth';
	import { loadAgentTheme, unloadAgentTheme } from '$lib/utils/a2ui-theme';
	import A2UISurfacePanel from '$lib/components/a2ui/A2UISurfacePanel.svelte';
	import A2UIWorkspaceNav from '$lib/components/a2ui/A2UIWorkspaceNav.svelte';

	const agentId = $derived(page.params.agentId ?? '');
	const agentSurfaces$ = $derived(agentId ? surfacesForAgent(agentId) : null);
	let agentSurfaces: import('$lib/stores/a2ui').A2UISurfaceInfo[] = $state([]);

	$effect(() => {
		if (!agentSurfaces$) { agentSurfaces = []; return; }
		const unsub = agentSurfaces$.subscribe((v) => { agentSurfaces = v; });
		return unsub;
	});

	// Load agent theme
	$effect(() => {
		if (agentId && agentSurfaces.length > 0) {
			loadAgentTheme(agentId);
			return () => unloadAgentTheme(agentId);
		}
	});

	onMount(() => {
		const wsClient = getWebSocketClient();
		const authState = get(auth);
		wsClient.connect(authState.token ?? undefined);

		// Init the a2ui store
		a2ui.init((action) => {
			const ctx = (action as any).context;
			const actionType = ctx?.type || ctx?.actionType;

			if (actionType === 'navigate' && ctx?.view) {
				wsClient.send('a2ui_navigate', {
					surfaceId: (action as any).surfaceId,
					targetView: ctx.view,
					params: ctx.params,
				});
			} else {
				wsClient.send('a2ui_action', action);
			}
		});

		const unsubA2UI = wsClient.on<{ surface_id: string; message: any }>('a2ui_message', (data) => {
			if (data?.message) {
				a2ui.processMessage(data.message);
			}
		});
		const unsubA2UIAction = wsClient.on<{ surfaceId: string; actionName: string; status: string }>('a2ui_action_status', (data) => {
			if (data) {
				a2ui.handleActionStatus(data);
			}
		});

		// Wait for WS connection, then request surface state
		const unsubStatus = wsClient.onStatus((status) => {
			if (status === 'connected' && agentId) {
				wsClient.send('a2ui_init', { agentId });
			}
		});

		return () => {
			unsubA2UI();
			unsubA2UIAction();
			unsubStatus();
		};
	});

	onDestroy(() => {
		a2ui.destroy();
	});
</script>

<div class="a2ui-popout-window" data-a2ui-agent={agentId}>
	{#if agentSurfaces.length > 0}
		{@const activeSurface = agentSurfaces[agentSurfaces.length - 1]}
		<A2UIWorkspaceNav
			{agentId}
			activeSurfaceId={activeSurface.surfaceId}
		/>
		<A2UISurfacePanel
			surfaceId={activeSurface.surfaceId}
			onClose={() => window.close()}
		/>
	{:else}
		<div class="a2ui-loading">
			<span class="loading loading-spinner loading-md"></span>
		</div>
	{/if}
</div>
