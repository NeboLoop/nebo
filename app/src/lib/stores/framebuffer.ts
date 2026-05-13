import { writable, derived } from 'svelte/store';
import { getWebSocketClient } from '$lib/websocket/client';
import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'FramebufferStore' });

/**
 * Framebuffer state
 */
export interface FramebufferState {
	isConnected: boolean;
	sessionId: string | null;
	width: number;
	height: number;
	fps: number;
	error: string | null;
}

const initialState: FramebufferState = {
	isConnected: false,
	sessionId: null,
	width: 0,
	height: 0,
	fps: 0,
	error: null
};

function createFramebufferStore() {
	const { subscribe, set, update } = writable<FramebufferState>(initialState);

	let worker: Worker | null = null;
	let wsUnsub: (() => void) | null = null;
	let frameCount = 0;
	let fpsInterval: ReturnType<typeof setInterval> | null = null;
	let lastFpsTimestamp = 0;

	/**
	 * Initialize the framebuffer worker.
	 */
	function initWorker(): Worker {
		if (worker) return worker;

		worker = new Worker('/framebufferWorker.js');

		worker.onerror = (e) => {
			const msg = e.message || 'Framebuffer worker error';
			update((s) => ({ ...s, error: msg }));
			log.error('Framebuffer worker error', e);
		};

		log.info('Framebuffer worker initialized');
		return worker;
	}

	/**
	 * Start FPS tracking.
	 */
	function startFpsTracking() {
		frameCount = 0;
		lastFpsTimestamp = performance.now();

		if (fpsInterval) clearInterval(fpsInterval);

		fpsInterval = setInterval(() => {
			const now = performance.now();
			const elapsed = (now - lastFpsTimestamp) / 1000;
			const fps = elapsed > 0 ? Math.round(frameCount / elapsed) : 0;
			update((s) => ({ ...s, fps }));
			frameCount = 0;
			lastFpsTimestamp = now;
		}, 1000);
	}

	/**
	 * Stop FPS tracking.
	 */
	function stopFpsTracking() {
		if (fpsInterval) {
			clearInterval(fpsInterval);
			fpsInterval = null;
		}
		update((s) => ({ ...s, fps: 0 }));
	}

	return {
		subscribe,

		/**
		 * Transfer an OffscreenCanvas to the worker for rendering.
		 */
		adoptCanvas(canvasId: string, offscreen: OffscreenCanvas): void {
			const w = initWorker();
			w.postMessage({ kind: 'adopt', canvasId, canvas: offscreen }, [offscreen]);
			log.debug('Canvas adopted: ' + canvasId);
		},

		/**
		 * Bind a session to a canvas for frame rendering.
		 */
		bindSession(sessionId: string, canvasId: string): void {
			const w = initWorker();
			w.postMessage({ kind: 'bind', sessionId, canvasId });

			// Subscribe to frame messages from WebSocket
			const ws = getWebSocketClient();
			if (wsUnsub) wsUnsub();

			wsUnsub = ws.on('frame', (data: {
				sessionId: string;
				seq: number;
				width: number;
				height: number;
				mimeType: string;
				data: number[];
			}) => {
				if (data.sessionId !== sessionId) return;

				frameCount++;

				// Convert data array to Uint8Array and forward to worker
				const frameData = new Uint8Array(data.data);
				w.postMessage({
					kind: 'frame',
					sessionId: data.sessionId,
					seq: data.seq,
					width: data.width,
					height: data.height,
					mimeType: data.mimeType,
					data: frameData
				});

				// Update dimensions if changed
				update((s) => ({
					...s,
					width: data.width,
					height: data.height
				}));
			});

			startFpsTracking();

			update((s) => ({
				...s,
				isConnected: true,
				sessionId,
				error: null
			}));

			log.info('Session bound: ' + sessionId + ' -> ' + canvasId);
		},

		/**
		 * Unbind a session from frame rendering.
		 */
		unbindSession(sessionId: string): void {
			if (worker) {
				worker.postMessage({ kind: 'unbind', sessionId });
			}

			if (wsUnsub) {
				wsUnsub();
				wsUnsub = null;
			}

			stopFpsTracking();

			update((s) => ({
				...s,
				isConnected: false,
				sessionId: null
			}));

			log.info('Session unbound: ' + sessionId);
		},

		/**
		 * Release a canvas from the worker.
		 */
		releaseCanvas(canvasId: string): void {
			if (worker) {
				worker.postMessage({ kind: 'release', canvasId });
			}
			log.debug('Canvas released: ' + canvasId);
		},

		/**
		 * Send a MessagePort to the worker for direct communication.
		 */
		setPort(port: MessagePort): void {
			const w = initWorker();
			w.postMessage({ kind: 'port', port }, [port]);
		},

		/**
		 * Clean up worker and all resources.
		 */
		cleanup(): void {
			if (wsUnsub) {
				wsUnsub();
				wsUnsub = null;
			}

			stopFpsTracking();

			if (worker) {
				worker.terminate();
				worker = null;
			}

			set(initialState);
			log.info('Framebuffer store cleaned up');
		},

		/**
		 * Set or clear the error message.
		 */
		setError(error: string | null): void {
			update((s) => ({ ...s, error }));
		}
	};
}

// Export the framebuffer store singleton
export const framebufferStore = createFramebufferStore();

// Derived stores for convenience
export const isFramebufferConnected = derived(framebufferStore, ($fb) => $fb.isConnected);
export const framebufferFps = derived(framebufferStore, ($fb) => $fb.fps);
export const framebufferError = derived(framebufferStore, ($fb) => $fb.error);
