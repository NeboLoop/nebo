/**
 * Base-aware navigation — the ONE goto pathway for the app.
 *
 * On desktop the SPA serves at the origin root (base "") and everything here is
 * a no-op passthrough. Opened through the loop's management tunnel it serves
 * under /t/<botID>/ with SvelteKit's runtime base injected by the hub, and
 * app-internal absolute paths ("/onboarding") would otherwise escape the prefix
 * onto the hub's own site. goto() re-roots outbound paths; appPath() strips the
 * base from $page.url.pathname so route checks compare app-relative paths.
 */
import { goto as kitGoto } from '$app/navigation';
import { base } from '$app/paths';

/** App-relative view of a full pathname (strips the runtime base prefix). */
export function appPath(pathname: string): string {
	if (base && pathname.startsWith(base)) {
		return pathname.slice(base.length) || '/';
	}
	return pathname;
}

/** Absolute app path → full path under the runtime base. */
export function withBase(path: string): string {
	if (base && path.startsWith('/') && !path.startsWith(base + '/') && path !== base) {
		return base + path;
	}
	return path;
}

/** Drop-in replacement for $app/navigation's goto that stays under the base. */
export const goto: typeof kitGoto = (url, opts) =>
	kitGoto(typeof url === 'string' ? withBase(url) : url, opts);
