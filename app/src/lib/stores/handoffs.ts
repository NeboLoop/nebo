// Hand-offs between employees: the trace the server keeps of work one
// employee passes to another (`GET /handoffs`). The sender's chat line and
// the receiving thread's header read it. A row is loaded once by id and kept
// current by `handoff_updated`; a hand-off still going is re-read every few
// seconds too, because an assignment's case ends where no event is sent.

import { writable, get } from 'svelte/store';
import type { HandoffView } from '$lib/api/neboComponents';
import { getWebSocketClient } from '$lib/websocket/client';

/** Every hand-off this screen has read, by id. */
export const handoffs = writable<Record<string, HandoffView>>({});

/** Statuses a hand-off still going is in. */
export function isLive(h: Pick<HandoffView, 'status'> | null | undefined): boolean {
	return h?.status === 'queued' || h?.status === 'running';
}

const POLL_MS = 5000;
let listening = false;
let poll: ReturnType<typeof setInterval> | null = null;
const loading = new Set<string>();

function put(rows: HandoffView[]): void {
	if (!rows.length) return;
	handoffs.update((all) => {
		const next = { ...all };
		for (const h of rows) next[h.id] = h;
		return next;
	});
	ensurePolling();
}

function listen(): void {
	if (listening) return;
	listening = true;
	getWebSocketClient().on('handoff_updated', (h: HandoffView) => {
		if (h?.id) put([h]);
	});
}

function ensurePolling(): void {
	const live = Object.values(get(handoffs)).some(isLive);
	if (live && !poll) {
		poll = setInterval(refreshLive, POLL_MS);
	} else if (!live && poll) {
		clearInterval(poll);
		poll = null;
	}
}

async function refreshLive(): Promise<void> {
	const ids = Object.values(get(handoffs)).filter(isLive).map((h) => h.id);
	await Promise.all(ids.map((id) => fetchOne(id)));
	ensurePolling();
}

async function fetchOne(id: string): Promise<void> {
	if (loading.has(id)) return;
	loading.add(id);
	try {
		const { getHandoff } = await import('$lib/api/nebo');
		const res = await getHandoff(id);
		if (res?.handoff) put([res.handoff, ...(res.descendants ?? [])]);
	} catch {
		// Unreadable now: the line keeps what it last knew.
	} finally {
		loading.delete(id);
	}
}

/** Read the hand-offs `ids` names that this screen has not read yet. */
export function trackHandoffs(ids: string[]): void {
	listen();
	const known = get(handoffs);
	for (const id of ids) {
		if (id && !known[id]) void fetchOne(id);
	}
}

/** The newest hand-off worked in conversation `sessionKey`, or null. */
export async function handoffInto(sessionKey: string): Promise<HandoffView | null> {
	listen();
	try {
		const { listHandoffs } = await import('$lib/api/nebo');
		const res = await listHandoffs(undefined, sessionKey, undefined, undefined, 1);
		const h = res?.handoffs?.[0] ?? null;
		if (h) put([h]);
		return h;
	} catch {
		return null;
	}
}

/** Where the receiving employee's work opens from the page at `here`: a
 *  message's thread in the view-only coworker transcript over the current
 *  page (`?cw=`, the same as a teammate thread); an assignment's case. */
export function receiverHref(h: HandoffView, here: URL): string {
	if (h.kind === 'message' && h.receiverSession) {
		const url = new URL(here);
		url.searchParams.set('cw', h.receiverSession);
		if (h.fromName) url.searchParams.set('cwf', h.fromName);
		return url.pathname + url.search;
	}
	return h.receiverLink;
}

/** The one line beneath a hand-off: its status, and what came back or why
 *  it failed. Labels are the caller's (`handoff.*`). */
export function statusLine(h: HandoffView, tr: (key: string, opts?: { values: Record<string, string> }) => string): string {
	const first = (s: string) => s.split('\n').map((l) => l.trim()).find(Boolean) ?? '';
	switch (h.status) {
		case 'done':
			return first(h.result) ? tr('handoff.doneWith', { values: { result: first(h.result) } }) : tr('handoff.status.done');
		case 'failed':
			return first(h.error) ? tr('handoff.failedWith', { values: { error: first(h.error) } }) : tr('handoff.status.failed');
		case 'stopped':
		case 'queued':
		case 'running':
			return tr(`handoff.status.${h.status}`);
		default:
			return '';
	}
}
