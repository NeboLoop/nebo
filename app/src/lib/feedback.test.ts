import { beforeAll, describe, it, expect } from 'vitest';
import { addMessages, init } from 'svelte-i18n';
import en from '$lib/i18n/locales/en.json';
import { feedbackForm, feedbackProblem, screenOf, sendFeedback, type FeedbackInput } from './feedback';

const png = (name = 'shot.png') => new File([new Uint8Array([137, 80, 78, 71])], name, { type: 'image/png' });

const input = (over: Partial<FeedbackInput> = {}): FeedbackInput => ({
	message: '  The inbox badge is wrong.  ',
	includeDiagnostics: true,
	screen: '/chat/assistant',
	files: [],
	...over
});

beforeAll(() => {
	addMessages('en', en);
	init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('provide feedback', () => {
	it('an empty message is not sent', async () => {
		const sent: FormData[] = [];
		expect(feedbackProblem({ message: '   ', files: [] })).toBe('Write a message first.');
		await expect(sendFeedback(input({ message: ' ' }), async (f) => (sent.push(f), { status: 'received' }))).rejects.toThrow(
			'Write a message first.'
		);
		expect(sent).toHaveLength(0);
		expect(feedbackProblem({ message: 'hi', files: [new File(['x'], 'a.pdf', { type: 'application/pdf' })] })).toBe(
			'Only images can be attached.'
		);
	});

	it('diagnostics on: the screen goes with it, as a path with no query or hash', () => {
		expect(screenOf('http://localhost:27895/chat/assistant?code=NEBO-abc&token=eyJx.y.z#frag')).toBe('/chat/assistant');
		const form = feedbackForm(input());
		expect(form.get('message')).toBe('The inbox badge is wrong.');
		expect(form.get('includeDiagnostics')).toBe('true');
		expect(form.get('screen')).toBe('/chat/assistant');
		const all = [...form.entries()].map(([k, v]) => `${k}=${typeof v === 'string' ? v : v.name}`).join('&');
		expect(all).not.toMatch(/eyJ|token|bearer/i);
	});

	it('diagnostics off: nothing about the screen is sent', () => {
		const form = feedbackForm(input({ includeDiagnostics: false }));
		expect(form.get('includeDiagnostics')).toBe('false');
		expect(form.has('screen')).toBe(false);
	});

	it('screenshots ride in the form as files for the upload path', async () => {
		const sent: FormData[] = [];
		await sendFeedback(input({ files: [png(), png('two.png')] }), async (f) => (sent.push(f), { status: 'received' }));
		const files = sent[0].getAll('file') as File[];
		expect(files.map((f) => f.name)).toEqual(['shot.png', 'two.png']);
	});

	it('a failure throws, so the form keeps the message and offers Retry', async () => {
		await expect(
			sendFeedback(input(), async () => {
				throw new Error('Could not send your feedback. Try again.');
			})
		).rejects.toThrow('Could not send your feedback. Try again.');
		await expect(sendFeedback(input(), async () => ({ status: 'received' }))).resolves.toBeUndefined();
	});
});
