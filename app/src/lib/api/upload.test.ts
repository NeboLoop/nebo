import { describe, it, expect } from 'vitest';
import { isStorageFull, uploadFailureMessage, uploadRefusal, UploadError } from './upload';

const full = JSON.stringify({
	error: "Your storage is full (2 GB). Uploads clear 30 days after they're added; the next space frees on November 4, 2026.",
	code: 'storage_full'
});

describe('a refused upload', () => {
	it('a full account is said plainly, in the server\'s words', () => {
		const e = uploadRefusal(413, full);
		expect(isStorageFull(e)).toBe(true);
		expect(uploadFailureMessage(e)).toBe(
			"Your storage is full (2 GB). Uploads clear 30 days after they're added; the next space frees on November 4, 2026."
		);
	});

	it('any other refusal keeps the upload-failed wording', () => {
		const tooBig = uploadRefusal(400, JSON.stringify({ error: 'That file is larger than 100 MB, which is the most one upload may carry.' }));
		expect(isStorageFull(tooBig)).toBe(false);
		expect(uploadFailureMessage(tooBig)).toBe(
			'File upload failed — message not sent. That file is larger than 100 MB, which is the most one upload may carry.'
		);
		const bare = uploadRefusal(502, '<html>bad gateway</html>');
		expect(bare).toBeInstanceOf(UploadError);
		expect(uploadFailureMessage(bare)).toBe('File upload failed — message not sent. Upload failed: 502');
		expect(uploadFailureMessage(new Error('Upload failed'))).toBe('File upload failed — message not sent. Upload failed');
	});
});
