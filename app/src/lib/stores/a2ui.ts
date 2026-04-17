/**
 * A2UI surface store — tracks active surfaces and provides a message processor.
 *
 * The MessageProcessor from @a2ui/web_core handles all protocol state:
 * surfaces, components, data models, actions. This store wraps it for
 * Svelte reactivity.
 */
import { writable, derived } from 'svelte/store';
import { MessageProcessor, type ActionListener } from '@a2ui/web_core/v0_9';
import type { LitComponentApi } from '@a2ui/lit/v0_9';
import type { A2uiMessage } from '@a2ui/web_core/v0_9';

/** Surface metadata tracked by the store (not the full SurfaceModel). */
export interface A2UISurfaceInfo {
	surfaceId: string;
	agentId: string;
	viewId: string;
}

/** Store state. */
export interface A2UIState {
	surfaces: Map<string, A2UISurfaceInfo>;
	processor: MessageProcessor<LitComponentApi> | null;
	/** Actions currently being processed by the backend (keyed by "surfaceId:actionName"). */
	pendingActions: Set<string>;
}

function createA2UIStore() {
	const { subscribe, update, set } = writable<A2UIState>({
		surfaces: new Map(),
		processor: null,
		pendingActions: new Set()
	});

	let processor: MessageProcessor<LitComponentApi> | null = null;
	let pendingMessages: A2uiMessage[] = [];
	let initPromise: Promise<void> | null = null;

	return {
		subscribe,

		/** Initialize the message processor with action handler. */
		async init(actionHandler: ActionListener) {
			// Dynamic import: Lit components use customElements.define()
			// which cannot run during SSR. init() is only called from onMount (client-side).
			initPromise = (async () => {
				const { neboCatalog } = await import('$lib/components/a2ui/nebo-catalog');
				processor = new MessageProcessor<LitComponentApi>([neboCatalog], actionHandler);

				// Track surface creation
				processor.onSurfaceCreated((surface) => {
					console.log('[a2ui] surface created:', surface.id);
					update((state) => {
						// Parse agent_id and view_id from surface ID format "agent:{agent_id}:{view_id}"
						const parts = surface.id.split(':');
						const agentId = parts.length >= 2 ? parts[1] : '';
						const viewId = parts.length >= 3 ? parts[2] : 'default';

						state.surfaces.set(surface.id, {
							surfaceId: surface.id,
							agentId,
							viewId
						});
						console.log('[a2ui] surfaces map now has', state.surfaces.size, 'entries');
						return { ...state, processor };
					});
				});

				// Track surface deletion
				processor.onSurfaceDeleted((id) => {
					update((state) => {
						state.surfaces.delete(id);
						return { ...state };
					});
				});

				update((state) => ({ ...state, processor }));

				// Flush any messages that arrived before init completed
				if (pendingMessages.length > 0) {
					console.log('[a2ui] flushing', pendingMessages.length, 'buffered messages');
					for (const msg of pendingMessages) {
						try {
							processor.processMessages([msg]);
						} catch (e) {
							const errMsg = e instanceof Error ? e.message : String(e);
							if (!errMsg.includes('already exists') && !errMsg.includes('not found')) {
								console.error('[a2ui] processMessage error (buffered):', e);
							}
						}
					}
					pendingMessages = [];
				}
			})();
			await initPromise;
		},

		/** Feed an A2UI message from the backend into the processor. */
		processMessage(message: A2uiMessage) {
			console.log('[a2ui] processMessage received:', message);
			if (processor) {
				try {
					processor.processMessages([message]);
				} catch (e) {
					// Ignore benign state errors (surface restored on reconnect, or
					// update for a surface that was closed client-side)
					const msg = e instanceof Error ? e.message : String(e);
					if (!msg.includes('already exists') && !msg.includes('not found')) {
						console.error('[a2ui] processMessage error:', e);
					}
				}
			} else {
				// Buffer messages until init() completes
				console.log('[a2ui] buffering message (processor not ready yet)');
				pendingMessages.push(message);
			}
		},

		/** Get the processor directly (for passing surface models to A2uiSurface). */
		getProcessor() {
			return processor;
		},

		/** Remove a single surface (e.g., user closed the panel). */
		removeSurface(surfaceId: string) {
			if (processor) {
				processor.model.deleteSurface(surfaceId);
			}
			update((state) => {
				state.surfaces.delete(surfaceId);
				return { ...state };
			});
		},

		/** Handle action status events from the backend. */
		handleActionStatus(data: { surfaceId: string; actionName: string; status: string }) {
			update((state) => {
				const key = `${data.surfaceId}:${data.actionName}`;
				if (data.status === 'processing') {
					state.pendingActions.add(key);
				} else {
					state.pendingActions.delete(key);
				}
				return { ...state, pendingActions: new Set(state.pendingActions) };
			});
		},

		/** Check if an action is currently being processed. */
		isActionPending(surfaceId: string, actionName: string): boolean {
			let pending = false;
			const unsub = subscribe((state) => {
				pending = state.pendingActions.has(`${surfaceId}:${actionName}`);
			});
			unsub();
			return pending;
		},

		/** Clean up on unmount. */
		destroy() {
			if (processor) {
				processor.model.dispose();
				processor = null;
			}
			set({ surfaces: new Map(), processor: null, pendingActions: new Set() });
		}
	};
}

export const a2ui = createA2UIStore();

/** Derived: list of active surface IDs. */
export const activeSurfaceIds = derived(a2ui, ($a2ui) => [...$a2ui.surfaces.keys()]);

/** Whether the workspace panel is open (shared between layout tab and Chat.svelte). */
export const workspaceOpen = writable(false);

/** Derived: surfaces for a specific agent. */
export function surfacesForAgent(agentId: string) {
	return derived(a2ui, ($a2ui) =>
		[...$a2ui.surfaces.values()].filter((s) => s.agentId === agentId)
	);
}
