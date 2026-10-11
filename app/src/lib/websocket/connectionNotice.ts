/**
 * What the owner is told about the app's connection to his bot.
 *
 * A dropped socket is routine (a phone in the background, wifi to cell, a
 * tunnel reset) and the client redials it on its own (./client.ts). A drop
 * the redials fix inside QUIET_MS is never mentioned: no line, no toast,
 * nothing that comes and goes. Only a connection that is still down once that
 * window has passed is reported, once, as the plain failure with Try again.
 * The redials keep running behind it, and the notice goes the moment the
 * connection is back.
 *
 * Try again dials at once. If that dial fails too, the failure shows again
 * straight away: the owner asked, and that is the answer.
 */

import type { ConnectionStatus } from './client';

/** How long a drop goes unmentioned while the redials run. */
export const QUIET_MS = 30_000;

/** The parts of the socket client the notice needs. */
export interface NoticeSocket {
	onStatus(handler: (status: ConnectionStatus) => void): () => void;
	getStatus(): ConnectionStatus;
	connect(): void;
}

/** Where the notice is shown: `show` puts the failure up with its Try again;
 *  `hide` takes it down (a no-op when it is not up). */
export interface NoticeView {
	show(tryAgain: () => void): void;
	hide(): void;
}

/** Watch the socket and keep the notice in step. Returns the stop function. */
export function watchConnection(ws: NoticeSocket, view: NoticeView, quietMs = QUIET_MS): () => void {
	let quiet: ReturnType<typeof setTimeout> | null = null;
	let shown = false;
	// The owner pressed Try again: the next failed dial is reported at once.
	let asked = false;

	const clearQuiet = () => {
		if (quiet) {
			clearTimeout(quiet);
			quiet = null;
		}
	};

	const tryAgain = () => {
		// The view takes its own notice down when Try again is pressed.
		shown = false;
		asked = true;
		ws.connect();
	};

	const show = () => {
		if (shown) return;
		shown = true;
		view.show(tryAgain);
	};

	const off = ws.onStatus((status) => {
		if (status === 'connected') {
			clearQuiet();
			asked = false;
			if (shown) {
				shown = false;
				view.hide();
			}
			return;
		}
		if (status !== 'error' && status !== 'disconnected') return;
		if (asked) {
			asked = false;
			show();
			return;
		}
		if (shown || quiet) return;
		quiet = setTimeout(() => {
			quiet = null;
			if (ws.getStatus() !== 'connected') show();
		}, quietMs);
	});

	return () => {
		off();
		clearQuiet();
		if (shown) {
			shown = false;
			view.hide();
		}
	};
}
