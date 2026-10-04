import { backendBase } from './base';
import { storage } from '$lib/storage';
import type { UploadedAttachment } from '$lib/types/attachment';
import type { LayerUploadResponse } from './neboComponents';

/** Longest edge the backend image gate normalizes to — converting here too
 *  keeps HEIC uploads small instead of shipping 12MP originals. */
const MAX_EDGE = 1568;

function isHeic(file: File): boolean {
	return (
		file.type === 'image/heic' ||
		file.type === 'image/heif' ||
		/\.hei[cf]$/i.test(file.name)
	);
}

/**
 * Convert HEIC/HEIF to JPEG in the browser BEFORE upload. The backend cannot
 * decode HEIC (patent-encumbered — no decoder ships in Nebo), but the devices
 * that produce HEIC can: iOS/macOS Safari decode it natively, so the sender
 * converts using Apple's own licensed decoder. Browsers that can't decode it
 * upload the original unchanged and the backend reports it honestly.
 */
async function convertHeicToJpeg(file: File): Promise<File> {
	try {
		const bitmap = await createImageBitmap(file);
		const scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
		const canvas = document.createElement('canvas');
		canvas.width = Math.max(1, Math.round(bitmap.width * scale));
		canvas.height = Math.max(1, Math.round(bitmap.height * scale));
		const ctx = canvas.getContext('2d');
		if (!ctx) return file;
		ctx.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
		bitmap.close();
		const blob = await new Promise<Blob | null>((res) => canvas.toBlob(res, 'image/jpeg', 0.85));
		if (!blob) return file;
		return new File([blob], file.name.replace(/\.hei[cf]$/i, '.jpg'), { type: 'image/jpeg' });
	} catch {
		return file;
	}
}

/** An upload the server refused. `code` is `storage_full` when the account's
 *  storage is full: the message is the server's own sentence, and the same
 *  upload is not worth offering again until space frees. */
export class UploadError extends Error {
	constructor(
		message: string,
		readonly status: number,
		readonly code?: string
	) {
		super(message);
		this.name = 'UploadError';
	}
}

export const STORAGE_FULL = 'storage_full';

export function isStorageFull(e: unknown): e is UploadError {
	return e instanceof UploadError && e.code === STORAGE_FULL;
}

/** The refusal a non-2xx upload answer carries: the server's `error` and
 *  `code` when it sent JSON, else the bare status. */
export function uploadRefusal(status: number, responseText: string): UploadError {
	try {
		const body = JSON.parse(responseText) as { error?: string; code?: string };
		if (body?.error) return new UploadError(body.error, status, body.code);
	} catch {
		// not JSON: fall through to the status
	}
	return new UploadError(`Upload failed: ${status}`, status);
}

/** What a failed upload says where it failed: a full account in the
 *  server's own words, anything else after "File upload failed". */
export function uploadFailureMessage(e: unknown): string {
	if (isStorageFull(e)) return e.message;
	return `File upload failed — message not sent. ${e instanceof Error ? e.message : ''}`.trim();
}

/** Where a file is landing: the employee it is for, and the conversation it
 *  came from. The backend announces every arrival as an event a flow can wait
 *  on (`attachment.audio` / `attachment.file`), and these are how that event
 *  says who and where. Omitted when there is no single employee behind the
 *  upload — a team post, say. */
export type UploadLanding = { agentId?: string; chatId?: string };

/**
 * Upload a file to NeboAI via the local server proxy.
 * HEIC/HEIF converts to JPEG first (see convertHeicToJpeg).
 * Uses XMLHttpRequest for upload progress tracking (fetch API doesn't support it).
 */
export async function uploadFile(
	file: File,
	landing?: UploadLanding,
	onProgress?: (percent: number) => void
): Promise<UploadedAttachment> {
	if (isHeic(file)) {
		file = await convertHeicToJpeg(file);
	}
	return new Promise((resolve, reject) => {
		// XMLHttpRequest on purpose: fetch exposes no upload-progress events, and
		// onProgress drives the composer's progress UI. Don't "modernize" to fetch.
		const xhr = new XMLHttpRequest();
		const formData = new FormData();
		formData.append('file', file);
		if (landing?.agentId) formData.append('agentId', landing.agentId);
		if (landing?.chatId) formData.append('chatId', landing.chatId);

		xhr.upload.addEventListener('progress', (e) => {
			if (e.lengthComputable) {
				onProgress?.(Math.round((e.loaded / e.total) * 100));
			}
		});

		xhr.addEventListener('load', () => {
			if (xhr.status >= 200 && xhr.status < 300) {
				try {
					resolve(JSON.parse(xhr.responseText));
				} catch {
					reject(new Error('Invalid upload response'));
				}
			} else {
				reject(uploadRefusal(xhr.status, xhr.responseText));
			}
		});

		xhr.addEventListener('error', () => reject(new Error('Upload failed')));
		xhr.addEventListener('abort', () => reject(new Error('Upload cancelled')));

		const token = storage.get('nebo_token');
		xhr.open('POST', `${backendBase()}/api/v1/files/upload`);
		if (token) xhr.setRequestHeader('Authorization', `Bearer ${token}`);
		xhr.send(formData);
	});
}

/**
 * Upload multiple files in parallel.
 * Returns uploaded attachments for all successful uploads.
 */
export async function uploadFiles(
	files: File[],
	landing?: UploadLanding,
	onProgress?: (index: number, percent: number) => void
): Promise<UploadedAttachment[]> {
	const results = await Promise.all(
		files.map((file, i) => uploadFile(file, landing, (pct) => onProgress?.(i, pct)))
	);
	return results;
}

/**
 * A layer pack (industry / franchise / company) as a .zip.
 *
 * Lives here rather than in the generated client because `POST /layers/upload`
 * takes multipart with the zip in the `file` field, and the generated client
 * speaks JSON only — the same reason `uploadFile` above is hand-rolled. A pack
 * given as a directory path IS JSON, so that one goes through the generated
 * `uploadLayerPack`. The response type is the generated one, so the shape still
 * comes from the Rust handler and never from here.
 */
export async function uploadLayerZip(file: File): Promise<LayerUploadResponse> {
	const form = new FormData();
	form.append('file', file);
	const headers: Record<string, string> = {};
	const token = storage.get('nebo_token');
	if (token) headers['Authorization'] = `Bearer ${token}`;
	// No Content-Type: the browser must set the multipart boundary itself.
	const response = await fetch(`${backendBase()}/api/v1/layers/upload`, {
		method: 'POST',
		credentials: 'include',
		headers,
		body: form
	});
	const text = await response.text();
	let parsed: unknown = {};
	try {
		parsed = text ? JSON.parse(text) : {};
	} catch {
		parsed = { error: text };
	}
	if (!response.ok) {
		const body = parsed as { error?: string; message?: string };
		throw new Error(body.error || body.message || `HTTP ${response.status}`);
	}
	return parsed as LayerUploadResponse;
}
