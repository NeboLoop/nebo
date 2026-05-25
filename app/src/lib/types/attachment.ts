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

export type AttachmentType = 'image' | 'video' | 'file';

export function getAttachmentType(mimeType: string): AttachmentType {
	if (mimeType.startsWith('image/')) return 'image';
	if (mimeType.startsWith('video/')) return 'video';
	return 'file';
}

export function formatFileSize(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
