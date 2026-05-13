/**
 * nebo.fetch() — mirrors native fetch() with auto-routing.
 *
 * Relative URLs → /apps/{id}/api/... (sidecar proxy)
 * Absolute external URLs → /apps/{id}/http/proxy (CORS-free outbound)
 */

import { getAppId, getBaseUrl } from './config';

export async function neboFetch(input: string, init?: RequestInit): Promise<Response> {
  const appId = getAppId();
  const base = getBaseUrl();

  // Absolute URL → proxy through Nebo for CORS-free access
  if (input.startsWith('http://') || input.startsWith('https://')) {
    const proxyUrl = `${base}/api/v1/apps/${appId}/http/proxy`;
    const headers: Record<string, string> = {};
    if (init?.headers) {
      const h = new Headers(init.headers);
      h.forEach((v, k) => { headers[k] = v; });
    }
    const proxyBody = {
      url: input,
      method: init?.method || 'GET',
      headers,
      body: init?.body ? String(init.body) : undefined
    };
    const resp = await fetch(proxyUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(proxyBody)
    });
    const data = await resp.json();
    // Reconstruct a Response-like object from the proxy result
    return new Response(data.body, {
      status: data.status,
      headers: data.headers
    });
  }

  // Relative URL → sidecar API
  const path = input.startsWith('/') ? input : `/${input}`;
  const url = `${base}/api/v1/apps/${appId}/api${path}`;
  return fetch(url, init);
}
