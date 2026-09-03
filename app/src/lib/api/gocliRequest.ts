import { backendBase } from './base';
import { storage } from '$lib/storage';
export type Method =
	| 'get'
	| 'GET'
	| 'delete'
	| 'DELETE'
	| 'head'
	| 'HEAD'
	| 'options'
	| 'OPTIONS'
	| 'post'
	| 'POST'
	| 'put'
	| 'PUT'
	| 'patch'
	| 'PATCH';

/**
 * Parse route parameters for responseType
 */
const reg = /:[a-z|A-Z]+/g;

export function parseParams(url: string): Array<string> {
	const ps = url.match(reg);
	if (!ps) {
		return [];
	}
	return ps.map((k) => k.replace(/:/, ''));
}

/**
 * Generate url and parameters
 * @param url
 * @param params
 */
export function genUrl(url: string, params: any) {
	if (!params) {
		return url;
	}

	const ps = parseParams(url);
	ps.forEach((k) => {
		const reg = new RegExp(`:${k}`);
		url = url.replace(reg, params[k]);
	});

	const path: Array<string> = [];
	for (const key of Object.keys(params)) {
		if (!ps.find((k) => k === key)) {
			path.push(`${key}=${params[key]}`);
		}
	}

	return url + (path.length > 0 ? `?${path.join('&')}` : '');
}

/**
 * Get API base URL from browser's current origin.
 */
function getBaseUrl(): string {
	if (typeof window !== 'undefined') {
		return backendBase();
	}
	// SSR fallback - relative URLs will work
	return '';
}

/**
 * Get auth token from localStorage
 */
function getAuthToken(): string | null {
	if (typeof window === 'undefined') return null;
	try {
		return storage.get('nebo_token');
	} catch {
		return null;
	}
}

/**
 * A read that has not answered in this long is treated as hung and retried
 * once. Over the management tunnel a request can stay open forever with no
 * error (a phone's first load sat on the startup spinner, 2026-09-03), and
 * an unbounded fetch turns that into a page that never resolves. Writes are
 * never retried and get a generous bound instead.
 */
export const READ_TIMEOUT_MS = 10_000;
export const WRITE_TIMEOUT_MS = 120_000;
export const RETRY_DELAY_MS = 200;

/**
 * Fetch and read the body under one abort timer, so a stalled body counts
 * the same as stalled headers. Reads (GET) get one retry after the timeout;
 * the stall is reported to the backend log so it can be seen from the server.
 */
export async function fetchBounded(
	apiUrl: string,
	init: RequestInit,
	fetchImpl: typeof fetch = fetch
): Promise<{ response: Response; text: string }> {
	const isRead = (init.method ?? 'GET').toUpperCase() === 'GET';
	// Set by the timer, so a stall can be told apart from a read that failed
	// outright (a socket reset mid-reconnect fails in milliseconds).
	let timedOut = false;
	const attempt = async () => {
		const controller = new AbortController();
		const outer = init.signal;
		if (outer) outer.addEventListener('abort', () => controller.abort(), { once: true });
		const timer = setTimeout(() => {
			timedOut = true;
			controller.abort();
		}, isRead ? READ_TIMEOUT_MS : WRITE_TIMEOUT_MS);
		try {
			const response = await fetchImpl(apiUrl, { ...init, signal: controller.signal });
			const text = await response.text();
			return { response, text };
		} finally {
			clearTimeout(timer);
		}
	};
	const started = Date.now();
	try {
		return await attempt();
	} catch (err) {
		if (!isRead || init.signal?.aborted) throw err;
		sendClientEvent(timedOut ? 'api_read_stalled' : 'api_read_failed', {
			detail: `${apiUrl.replace(getBaseUrl(), '')} ${timedOut ? '' : String(err)}`.trim(),
			durationMs: Date.now() - started
		});
		await new Promise((r) => setTimeout(r, RETRY_DELAY_MS));
		return attempt();
	}
}

/**
 * One line in the backend log for a client-side connection event. Sits in
 * the transport layer (not the generated client) because the generated
 * client imports this module; a plain fetch here is the one exception.
 * Never throws and never retries: telemetry must not cause what it reports.
 */
export function sendClientEvent(
	event: string,
	fields: { detail?: string; durationMs?: number; code?: number } = {}
): void {
	if (typeof window === 'undefined') return;
	const body = JSON.stringify({ event, page: window.location.pathname, ...fields });
	const headers: Record<string, string> = { 'Content-Type': 'application/json' };
	const token = getAuthToken();
	if (token) headers['Authorization'] = `Bearer ${token}`;
	fetch(`${getBaseUrl()}/api/v1/client/events`, { method: 'POST', credentials: 'include', headers, body, keepalive: true }).catch(
		() => {
			/* the log line is best-effort */
		}
	);
}

export async function request({
	method,
	url,
	data,
	config = {}
}: {
	method: Method;
	url: string;
	data?: unknown;
	config?: unknown;
}) {
	// Get API base URL from browser origin
	const apiUrl = `${getBaseUrl()}${url}`;

	// Build headers with auth token if available
	const headers: Record<string, string> = {
		'Content-Type': 'application/json'
	};

	const token = getAuthToken();
	if (token) {
		headers['Authorization'] = `Bearer ${token}`;
	}

	const { response, text } = await fetchBounded(apiUrl, {
		method: method.toLocaleUpperCase(),
		credentials: 'include',
		headers,
		body: data ? JSON.stringify(data) : undefined,
		// @ts-ignore
		...config
	});

	let parsedData;
	try {
		parsedData = text ? JSON.parse(text) : {};
		// Handle null response body
		if (parsedData === null) {
			parsedData = {};
		}
	} catch {
		// API returned non-JSON response, use the text as the error message
		parsedData = { message: text || 'Request failed' };
	}

	// Check if the response indicates an error
	if (!response.ok || (parsedData.code && parsedData.code >= 400)) {
		const error = new Error(parsedData.message || parsedData.error || `HTTP ${response.status}`);
		// @ts-ignore
		error.response = {
			status: response.status,
			data: parsedData
		};
		throw error;
	}

	return parsedData;
}

function api<T>(method: Method = 'get', url: string, req: any, config?: unknown): Promise<T> {
	if (url.match(/:/) || method.match(/get|delete/i)) {
		url = genUrl(url, req?.params || req?.forms);
	}
	method = method.toLocaleLowerCase() as Method;

	switch (method) {
		case 'get':
			return request({ method: 'get', url, data: undefined, config });
		case 'delete':
			return request({ method: 'delete', url, data: undefined, config });
		case 'put':
			return request({ method: 'put', url, data: req, config });
		case 'post':
			return request({ method: 'post', url, data: req, config });
		case 'patch':
			return request({ method: 'patch', url, data: req, config });
		default:
			return request({ method: 'post', url, data: req, config });
	}
}

/**
 * Request that returns a blob (for binary responses like audio)
 */
export async function requestBlob({
	method,
	url,
	data
}: {
	method: Method;
	url: string;
	data?: unknown;
}): Promise<Blob> {
	const apiUrl = `${getBaseUrl()}${url}`;

	const headers: Record<string, string> = {
		'Content-Type': 'application/json'
	};

	const token = getAuthToken();
	if (token) {
		headers['Authorization'] = `Bearer ${token}`;
	}

	const response = await fetch(apiUrl, {
		method: method.toLocaleUpperCase(),
		credentials: 'include',
		headers,
		body: data ? JSON.stringify(data) : undefined
	});

	if (!response.ok) {
		const text = await response.text();
		const error = new Error(text || `HTTP ${response.status}`);
		// @ts-ignore
		error.response = { status: response.status };
		throw error;
	}

	return response.blob();
}

// In-flight GET request deduplication: if the same URL is already being fetched,
// return the existing promise instead of making a duplicate request.
const inflightGets = new Map<string, Promise<unknown>>();

export const webapi = {
	get<T>(url: string, params?: any, req?: any): Promise<T> {
		// For GET requests, append params as query string to URL
		if (params) {
			const searchParams = new URLSearchParams();
			Object.entries(params).forEach(([key, value]) => {
				if (value !== undefined && value !== null) {
					searchParams.append(key, String(value));
				}
			});
			const queryString = searchParams.toString();
			if (queryString) {
				url += (url.includes('?') ? '&' : '?') + queryString;
			}
		}
		const existing = inflightGets.get(url);
		if (existing) return existing as Promise<T>;
		const p = api<T>('get', url, undefined, req).finally(() => inflightGets.delete(url));
		inflightGets.set(url, p);
		return p;
	},
	delete<T>(url: string, params?: any, req?: any): Promise<T> {
		// DELETE carries no body — params must travel in the query string (same
		// as get) so they reach the backend's Query extractor.
		if (params) {
			const searchParams = new URLSearchParams();
			Object.entries(params).forEach(([key, value]) => {
				if (value !== undefined && value !== null) {
					searchParams.append(key, String(value));
				}
			});
			const queryString = searchParams.toString();
			if (queryString) {
				url += (url.includes('?') ? '&' : '?') + queryString;
			}
		}
		return api<T>('delete', url, undefined, req) as Promise<T>;
	},
	put<T>(url: string, params?: any, req?: any): Promise<T> {
		return api<T>(
			'put',
			url,
			{
				...(params || {}),
				...(req || {})
			},
			req
		) as Promise<T>;
	},
	post<T>(url: string, params?: any, req?: any): Promise<T> {
		return api<T>(
			'post',
			url,
			{
				...(params || {}),
				...(req || {})
			},
			req
		) as Promise<T>;
	},
	patch<T>(url: string, params?: any, req?: any): Promise<T> {
		return api<T>(
			'patch',
			url,
			{
				...(params || {}),
				...(req || {})
			},
			req
		) as Promise<T>;
	},
	postBlob(url: string, data?: any): Promise<Blob> {
		return requestBlob({ method: 'post', url, data });
	}
};

export default webapi;
