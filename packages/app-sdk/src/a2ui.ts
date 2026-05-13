/**
 * nebo.a2ui — bridge between Nebo's WebSocket transport and @a2ui/web_core.
 *
 * The SDK handles:
 * - Receiving A2UI v0.9 messages from the backend WebSocket
 * - Feeding them to the processor via processMessages()
 * - Sending actions back to the agent via the WebSocket
 *
 * Usage (React example):
 *
 *   import { nebo } from '@neboai/app-sdk';
 *   import { MessageProcessor } from '@a2ui/web_core/v0_9';
 *   import { basicCatalog } from '@a2ui/react/v0_9';
 *   import { A2uiSurface } from '@a2ui/react/v0_9';
 *
 *   const processor = new MessageProcessor([basicCatalog]);
 *   nebo.a2ui.init(processor);
 *   nebo.surfaces.connect();
 */

import type { A2uiMessage, A2uiMessageListWrapper } from '@a2ui/web_core/v0_9';

/** Structural type for @a2ui/web_core MessageProcessor — matches processMessages signature exactly. */
export interface A2UIMessageProcessor {
	processMessages(messages: A2uiMessage[] | A2uiMessageListWrapper): void;
}

export class NeboA2UI {
	private processor: A2UIMessageProcessor | null = null;
	private sendFn: ((data: string) => void) | null = null;

	/**
	 * Initialize with a @a2ui/web_core MessageProcessor.
	 * Call before nebo.surfaces.connect().
	 */
	init(processor: A2UIMessageProcessor): void {
		this.processor = processor;
	}

	/** Whether a processor has been initialized */
	get initialized(): boolean {
		return this.processor !== null;
	}

	/**
	 * @internal Called by surfaces module when an a2ui_message arrives.
	 */
	_handleMessage(message: unknown): void {
		if (!this.processor) return;
		try {
			this.processor.processMessages([message as A2uiMessage]);
		} catch (e) {
			console.error('[nebo-sdk] A2UI processing error:', e);
		}
	}

	/**
	 * @internal Called by SDK to provide WebSocket send capability.
	 */
	_setSendFn(fn: (data: string) => void): void {
		this.sendFn = fn;
	}

	/**
	 * Send a v0.9 client action back to the agent.
	 */
	sendAction(surfaceId: string, action: Record<string, unknown>): void {
		if (!this.sendFn) return;
		this.sendFn(JSON.stringify({
			type: 'a2ui_action',
			data: {
				surface_id: surfaceId,
				message: {
					version: 'v0.9',
					action: {
						surfaceId,
						timestamp: new Date().toISOString(),
						...action,
					},
				},
			},
		}));
	}

	/**
	 * Send a v0.9 client error report back to the agent.
	 */
	sendError(surfaceId: string, code: string, message: string, path?: string): void {
		if (!this.sendFn) return;
		this.sendFn(JSON.stringify({
			type: 'a2ui_action',
			data: {
				surface_id: surfaceId,
				message: {
					version: 'v0.9',
					error: { code, surfaceId, message, path },
				},
			},
		}));
	}
}

export const a2ui = new NeboA2UI();
