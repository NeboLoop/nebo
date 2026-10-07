/**
 * What the panel needs to know about a sheet viewer's saves, by document —
 * the panel never reaches into the viewer.
 *
 * - Unsaved edits: a SheetView with edits registers a flush, so Download can
 *   save first (the file must carry the owner's edits).
 * - Own versions: a version the viewer wrote itself (Save, autosave) already
 *   matches what it shows, so the panel must not remount it when that version
 *   arrives (that would drop the selection, scroll and undo stack every save).
 */
import { backendBase } from '$lib/api/base';

const flushes = new Map<string, () => Promise<void>>();
const inFlight = new Map<string, number>();
const own = new Set<string>();

export function setPendingSave(documentId: string, flush: (() => Promise<void>) | null): void {
	if (flush) flushes.set(documentId, flush);
	else flushes.delete(documentId);
}

export function hasPendingSave(documentId: string): boolean {
	return flushes.has(documentId);
}

/** A save request is going out (its version may arrive before its response). */
export function beginSave(documentId: string): void {
	inFlight.set(documentId, (inFlight.get(documentId) ?? 0) + 1);
}

export function endSave(documentId: string, version: number | null): void {
	const n = (inFlight.get(documentId) ?? 1) - 1;
	if (n > 0) inFlight.set(documentId, n);
	else inFlight.delete(documentId);
	if (version != null) own.add(`${documentId}:${version}`);
}

/** True for a version this session's viewer wrote (or is writing right now). */
export function isOwnVersion(documentId: string, version: number): boolean {
	return own.has(`${documentId}:${version}`) || (inFlight.get(documentId) ?? 0) > 0;
}

/**
 * Save the document's unsaved edits (if any) and return the URL of its
 * latest version — what Download should fetch. Null when the lookup fails.
 */
export async function flushPendingSave(documentId: string): Promise<string | null> {
	await flushes.get(documentId)?.();
	try {
		const res = await fetch(`${backendBase()}/api/v1/work/documents?id=${encodeURIComponent(documentId)}`);
		const body = await res.json();
		return body?.documents?.[0]?.url ?? null;
	} catch {
		return null;
	}
}

/**
 * The key the panel mounts a document's viewer under. A different document,
 * version or view mode remounts it — except a version the viewer wrote itself,
 * which keeps the previous key (the viewer already shows that content).
 */
export function viewerKey(prev: string, documentId: string, version: number, mode: string): string {
	const sameDocAndMode = prev.startsWith(`${documentId}:`) && prev.endsWith(`:${mode}`);
	if (sameDocAndMode && isOwnVersion(documentId, version)) return prev;
	return `${documentId}:${version}:${mode}`;
}
