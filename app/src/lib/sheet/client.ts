/**
 * Typed client for the sheet engine endpoints (one per verb):
 *   GET  /api/v1/work/{documentId}/sheet?version=N[&rows=a-b&sheet=Name&session=id]
 *   POST /api/v1/work/{documentId}/sheet/edit  {session, version, edits}
 *   POST /api/v1/work/{documentId}/sheet/save  {session, agentId?, sessionKey?}
 * A 409 means the document moved on since the session's version, and a 404
 * on edit/save that the session is gone; both messages say to reload.
 */
import { backendBase } from '$lib/api/base';
import type { EditResponse, SaveResponse, SheetEdit, SheetViewModel } from './types';

export class SheetApiError extends Error {
	constructor(
		public status: number,
		message: string
	) {
		super(message);
	}
}

function base(documentId: string): string {
	return `${backendBase()}/api/v1/work/${encodeURIComponent(documentId)}/sheet`;
}

async function json<T>(res: Response): Promise<T> {
	if (!res.ok) {
		let message = `${res.status}`;
		try {
			const body = await res.json();
			message = body?.error || body?.message || message;
		} catch {
			/* not JSON */
		}
		throw new SheetApiError(res.status, message);
	}
	return res.json() as Promise<T>;
}

/**
 * The view model. `rows` pages one row range (1-based, inclusive); `sheet`
 * limits cells to that sheet (the others come back without cells). Without a
 * `session` a new edit session starts and its id comes back; with one, the
 * view shows that session's edits so far.
 */
export async function getSheet(
	documentId: string,
	opts: { version?: number; rows?: [number, number]; sheet?: string; session?: string } = {}
): Promise<SheetViewModel> {
	const q = new URLSearchParams();
	if (opts.version != null) q.set('version', String(opts.version));
	if (opts.rows) q.set('rows', `${opts.rows[0]}-${opts.rows[1]}`);
	if (opts.sheet) q.set('sheet', opts.sheet);
	if (opts.session) q.set('session', opts.session);
	const qs = q.toString();
	return json<SheetViewModel>(await fetch(`${base(documentId)}${qs ? `?${qs}` : ''}`));
}

/** `version` is the version the session is over; it must be the document's latest. */
export async function editSheet(documentId: string, session: string, version: number, edits: SheetEdit[]): Promise<EditResponse> {
	const res = await fetch(`${base(documentId)}/edit`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ session, version, edits })
	});
	const body = await json<Partial<EditResponse>>(res);
	return { changed: body.changed ?? [], errors: body.errors ?? [] };
}

/**
 * Write the session's edits as a new version (the current one when nothing
 * changed). `agentId`/`sessionKey` place the "Saved …" message that announces
 * the version in the chat, as restore_version does. `keepalive` lets the
 * request outlive the view that sent it (the panel closing).
 */
export async function saveSheet(
	documentId: string,
	session: string,
	opts: { agentId?: string; sessionKey?: string; keepalive?: boolean } = {}
): Promise<SaveResponse> {
	const res = await fetch(`${base(documentId)}/save`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ session, agentId: opts.agentId || undefined, sessionKey: opts.sessionKey || undefined }),
		keepalive: opts.keepalive ?? false
	});
	return json<SaveResponse>(res);
}
