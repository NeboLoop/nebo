import { backendBase } from './base';
import { storage } from '$lib/storage';
import type { UploadedAttachment } from '$lib/types/attachment';

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

/**
 * Upload a file to NeboAI via the local server proxy.
 * HEIC/HEIF converts to JPEG first (see convertHeicToJpeg).
 * Uses XMLHttpRequest for upload progress tracking (fetch API doesn't support it).
 */
export async function uploadFile(
	file: File,
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
				reject(new Error(`Upload failed: ${xhr.status}`));
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
	onProgress?: (index: number, percent: number) => void
): Promise<UploadedAttachment[]> {
	const results = await Promise.all(
		files.map((file, i) => uploadFile(file, (pct) => onProgress?.(i, pct)))
	);
	return results;
}
