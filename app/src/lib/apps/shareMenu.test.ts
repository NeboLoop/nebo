import { describe, expect, it } from 'vitest';
import { linkFile, shareEntries } from './shareMenu';

describe('shareEntries', () => {
	const entry = { label: 'Make it an app', say: 'Make this design a Nebo app.' };
	const link = { label: 'Share a link', say: 'Share this design as a link.', share: true };

	it("lists the app's entries in a chat the owner can send in", () => {
		expect(shareEntries({ shareMenu: [entry] }, true)).toEqual([entry]);
	});

	it('lists a link entry beside the others, keeping that it asks for a link', () => {
		expect(shareEntries({ shareMenu: [link, entry] }, true)).toEqual([link, entry]);
		expect(shareEntries({ shareMenu: [link] }, true)[0].share).toBe(true);
	});

	it('lists nothing when the app declares none, or from a bot without the field', () => {
		expect(shareEntries({ shareMenu: [] }, true)).toEqual([]);
		expect(shareEntries({}, true)).toEqual([]);
		expect(shareEntries(null, true)).toEqual([]);
	});

	it('lists nothing in a chat the owner cannot send in', () => {
		expect(shareEntries({ shareMenu: [entry] }, false)).toEqual([]);
	});

	it('skips an entry with nothing to show or say', () => {
		expect(shareEntries({ shareMenu: [{ label: ' ', say: 'x' }, entry, { label: 'Export', say: '' }] }, true)).toEqual([entry]);
	});
});

describe('linkFile: the file a link ask is answered with', () => {
	const before = [
		{ type: 'user', content: 'Design a flyer' },
		{ type: 'assistant', content: 'Here it is', attachments: [{ url: '/api/v1/files/.shared/aa/old.png', filename: 'old.png' }] },
	];
	const ask = { type: 'user', content: 'Share this design as a link.' };
	const from = before.length;

	it('is nothing while the answer has handed over no file', () => {
		expect(linkFile([...before, ask], from)).toBeNull();
		expect(linkFile([...before, ask, { type: 'assistant', content: 'Making the PNG…' }], from)).toBeNull();
	});

	it('is a picture or clip the answer shared', () => {
		const png = { type: 'assistant', content: 'Here is the PNG', attachments: [{ url: '/api/v1/files/.shared/bb/Flyer.png', filename: 'Flyer.png' }] };
		expect(linkFile([...before, ask, png], from)).toEqual({ url: '/api/v1/files/.shared/bb/Flyer.png', title: 'Flyer.png' });
	});

	it('is a document the answer wrote, and the last file when there are several', () => {
		const two = {
			type: 'assistant',
			content: 'Done',
			attachments: [{ url: '/api/v1/files/.shared/cc/frame.png', filename: 'frame.png' }],
			workItems: [{ url: '/api/v1/files/.shared/dd/Launch.html', title: 'Launch.html' }],
		};
		expect(linkFile([...before, ask, two], from)).toEqual({ url: '/api/v1/files/.shared/dd/Launch.html', title: 'Launch.html' });
	});

	it('reads a file served at a full address as its Work reference', () => {
		const mp4 = { type: 'assistant', content: '', attachments: [{ url: 'http://localhost:27895/api/v1/files/.shared/ee/Reel%20cut.mp4', filename: '' }] };
		expect(linkFile([...before, ask, mp4], from)).toEqual({ url: '/api/v1/files/.shared/ee/Reel%20cut.mp4', title: 'Reel cut.mp4' });
	});

	it('never takes a file from before the ask, after the next message, or outside Work', () => {
		const later = [{ type: 'user', content: 'Now make it blue' }, { type: 'assistant', content: '', attachments: [{ url: '/api/v1/files/.shared/ff/blue.png', filename: 'blue.png' }] }];
		expect(linkFile([...before, ask, { type: 'assistant', content: 'No file' }, ...later], from)).toBeNull();
		expect(linkFile([...before, ask, { type: 'assistant', content: '', attachments: [{ url: 'https://cdn.example.com/x.png', filename: 'x.png' }] }], from)).toBeNull();
	});
});
