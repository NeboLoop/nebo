import type { UploadedAttachment } from '$lib/types/attachment';

/**
 * Upload a file to NeboLoop via the local server proxy.
 * Uses XMLHttpRequest for upload progress tracking (fetch API doesn't support it).
 */
export function uploadFile(
	file: File,
	onProgress?: (percent: number) => void
): Promise<UploadedAttachment> {
	return new Promise((resolve, reject) => {
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

		const token = localStorage.getItem('nebo_token');
		xhr.open('POST', `${window.location.origin}/api/v1/files/upload`);
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
