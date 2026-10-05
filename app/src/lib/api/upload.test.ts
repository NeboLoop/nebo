import { beforeAll, describe, it, expect } from 'vitest';
import { addMessages, init, locale } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';
import { isStorageFull, storageFullMessage, uploadFailureMessage, uploadRefusal, UploadError } from './upload';

beforeAll(async () => {
	addMessages('en', en);
	addMessages('de', { chat: { storageFull: 'Dein Speicher ist voll ({cap}). Uploads werden nach {days} Tagen gelöscht.', storageNextFree: 'Der nächste Platz wird am {date} frei.' } });
	await init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const full = JSON.stringify({
	error: "Your storage is full (2 GB). Uploads clear 15 days after they're added; the next space frees on November 4, 2026.",
	code: 'storage_full'
});

describe('a refused upload', () => {
	it('a full account is said plainly: cap, keeping period and the next free day', () => {
		const e = uploadRefusal(413, full);
		expect(isStorageFull(e)).toBe(true);
		expect(uploadFailureMessage(e)).toBe(
			"Your storage is full (2 GB). Uploads clear 15 days after they're added. The next space frees on November 4, 2026."
		);
	});

	it('a full account without a cap or a date still reads as one sentence', () => {
		expect(storageFullMessage('Your storage is full.')).toBe(
			"Your storage is full. Uploads clear 15 days after they're added."
		);
	});

	it('a full account is said in the owner\'s language', async () => {
		await locale.set('de');
		try {
			expect(uploadFailureMessage(uploadRefusal(413, full))).toBe(
				'Dein Speicher ist voll (2 GB). Uploads werden nach 15 Tagen gelöscht. Der nächste Platz wird am 4. November 2026 frei.'
			);
		} finally {
			await locale.set('en');
		}
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
