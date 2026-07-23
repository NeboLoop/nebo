/** Metadata returned by POST /api/v1/files/upload */
export interface UploadedAttachment {
	fileId: string;
	filename: string;
	mimeType: string;
	size: number;
	url: string;
	thumbnailUrl?: string;
	width?: number;
	height?: number;
	duration?: number;
}

export type AttachmentType = 'image' | 'video' | 'audio' | 'file';

export function getAttachmentType(mimeType: string): AttachmentType {
	if (mimeType.startsWith('image/')) return 'image';
	if (mimeType.startsWith('video/')) return 'video';
	if (mimeType.startsWith('audio/')) return 'audio';
	return 'file';
}

/**
 * URL that actually renders an uploaded attachment. The loop's `/files/{id}`
 * is auth-gated — a bare <img src> can't attach a bearer token — so media
 * streams through the local server's authenticated proxy instead.
 */
export function attachmentMediaUrl(att: UploadedAttachment, base: string): string {
	return `${base}/api/v1/comm-files/${att.fileId}?mime=${encodeURIComponent(att.mimeType)}`;
}

export function formatFileSize(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
