import { backendBase } from '$lib/api/base';
import { storage } from '$lib/storage';
import type { SendFeedbackResponse } from '$lib/api/neboComponents';

/**
 * Feedback to the NeboAI team, from the account menu. The form posts to this
 * Nebo's `POST /api/v1/neboai/feedback`, which uploads the screenshots through
 * the one file path (the hub's `POST /api/v1/files/upload`), adds the redacted
 * diagnostics when they are on, and files it with the hub's support inbox.
 *
 * Multipart, so it is sent here rather than through the generated client,
 * which speaks JSON only — the same reason `uploadFile` is hand-rolled. The
 * response type is the generated one.
 */

/** The longest message the hub takes. */
export const FEEDBACK_MESSAGE_MAX = 5000;
/** The most screenshots one piece of feedback carries. */
export const FEEDBACK_MAX_ATTACHMENTS = 5;

export interface FeedbackInput {
	message: string;
	includeDiagnostics: boolean;
	/** Where the person was: the route's path, never its query or hash. */
	screen: string;
	files: File[];
}

/** Why the form will not send yet, or null. */
export function feedbackProblem(input: Pick<FeedbackInput, 'message' | 'files'>): string | null {
	const message = input.message.trim();
	if (!message) return 'Write a message first.';
	if (message.length > FEEDBACK_MESSAGE_MAX) return `Keep the message under ${FEEDBACK_MESSAGE_MAX} characters.`;
	if (input.files.length > FEEDBACK_MAX_ATTACHMENTS) return `Attach at most ${FEEDBACK_MAX_ATTACHMENTS} screenshots.`;
	if (input.files.some((f) => !f.type.startsWith('image/'))) return 'Only images can be attached.';
	return null;
}

/** A route as the diagnostics name it: the path alone. A query or hash can
 *  carry a code or a token; the path is enough to say where. */
export function screenOf(url: URL | string): string {
	const u = typeof url === 'string' ? new URL(url, 'http://local') : url;
	return u.pathname;
}

/** The multipart body. With diagnostics off, nothing about the machine or the
 *  screen is sent. */
export function feedbackForm(input: FeedbackInput): FormData {
	const form = new FormData();
	form.append('message', input.message.trim());
	form.append('includeDiagnostics', input.includeDiagnostics ? 'true' : 'false');
	if (input.includeDiagnostics) form.append('screen', input.screen);
	for (const f of input.files) form.append('file', f, f.name);
	return form;
}

export type FeedbackPost = (form: FormData) => Promise<SendFeedbackResponse>;

async function post(form: FormData): Promise<SendFeedbackResponse> {
	const headers: Record<string, string> = {};
	const token = storage.get('nebo_token');
	if (token) headers['Authorization'] = `Bearer ${token}`;
	// No Content-Type: the browser sets the multipart boundary itself.
	const response = await fetch(`${backendBase()}/api/v1/neboai/feedback`, {
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
	return parsed as SendFeedbackResponse;
}

/** Sends the feedback. Throws when it was not sent; the form keeps what the
 *  person typed and offers Retry. */
export async function sendFeedback(input: FeedbackInput, send: FeedbackPost = post): Promise<void> {
	const problem = feedbackProblem(input);
	if (problem) throw new Error(problem);
	await send(feedbackForm(input));
}
