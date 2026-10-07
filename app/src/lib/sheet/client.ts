/**
 * Typed client for the sheet engine endpoints (one per verb):
 *   GET  /api/v1/work/{documentId}/sheet?version=N[&rows=a-b&sheet=Name]
 *   POST /api/v1/work/{documentId}/sheet/edit  {session, edits}
 *   POST /api/v1/work/{documentId}/sheet/save  {session}
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

/** The view model. `rows` pages one row range (1-based, inclusive) of `sheet`, or of every sheet when no sheet is named. */
export async function getSheet(
	documentId: string,
	opts: { version?: number; rows?: [number, number]; sheet?: string } = {}
): Promise<SheetViewModel> {
	const q = new URLSearchParams();
	if (opts.version != null) q.set('version', String(opts.version));
	if (opts.rows) q.set('rows', `${opts.rows[0]}-${opts.rows[1]}`);
	if (opts.sheet) q.set('sheet', opts.sheet);
	const qs = q.toString();
	return json<SheetViewModel>(await fetch(`${base(documentId)}${qs ? `?${qs}` : ''}`));
}

export async function editSheet(documentId: string, session: string, edits: SheetEdit[]): Promise<EditResponse> {
	const res = await fetch(`${base(documentId)}/edit`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ session, edits })
	});
	const body = await json<Partial<EditResponse>>(res);
	return { changed: body.changed ?? [], errors: body.errors ?? [] };
}

/** Write the session's edits as a new version. `keepalive` lets the request
 *  outlive the page section that sent it (the panel closing). */
export async function saveSheet(documentId: string, session: string, keepalive = false): Promise<SaveResponse> {
	const res = await fetch(`${base(documentId)}/save`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ session }),
		keepalive
	});
	return json<SaveResponse>(res);
}
