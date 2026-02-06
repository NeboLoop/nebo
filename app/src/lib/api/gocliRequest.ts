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
		return window.location.origin;
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
		return localStorage.getItem('nebo_token');
	} catch {
		return null;
	}
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

	const response = await fetch(apiUrl, {
		method: method.toLocaleUpperCase(),
		credentials: 'include',
		headers,
		body: data ? JSON.stringify(data) : undefined,
		// @ts-ignore
		...config
	});

	const text = await response.text();
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
		return api<T>('get', url, undefined, req) as Promise<T>;
	},
	delete<T>(url: string, params?: any, req?: any): Promise<T> {
		return api<T>(
			'delete',
			url,
			{
				...(params || {}),
				...(req || {})
			},
			req
		) as Promise<T>;
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
