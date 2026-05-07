/**
 * Auto-detect the app ID and base URL.
 *
 * Production: URL path is /apps/{id}/ui/... → extract id from path.
 * Dev (Vite): <meta name="nebo-app-id" content="my-app"> in index.html.
 */

let _appId: string | null = null;
let _baseUrl: string | null = null;

export function getAppId(): string {
  if (_appId) return _appId;

  // Try URL path first: /apps/{id}/ui/...
  const match = window.location.pathname.match(/^\/apps\/([^/]+)\/ui/);
  if (match) {
    _appId = match[1];
    return _appId;
  }

  // Try meta tag (dev mode)
  const meta = document.querySelector('meta[name="nebo-app-id"]');
  if (meta) {
    _appId = meta.getAttribute('content') || '';
    return _appId;
  }

  throw new Error(
    '[nebo-sdk] Cannot detect app ID. In production, serve from /apps/{id}/ui/. ' +
    'In dev, add <meta name="nebo-app-id" content="your-id"> to index.html.'
  );
}

export function getBaseUrl(): string {
  if (_baseUrl) return _baseUrl;

  // In production (same origin), base is just the origin
  // In dev, check for nebo-base-url meta tag
  const meta = document.querySelector('meta[name="nebo-base-url"]');
  if (meta) {
    _baseUrl = meta.getAttribute('content') || '';
    return _baseUrl;
  }

  _baseUrl = window.location.origin;
  return _baseUrl;
}

export function setAppId(id: string): void {
  _appId = id;
}

export function setBaseUrl(url: string): void {
  _baseUrl = url;
}
