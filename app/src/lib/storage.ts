/**
 * Base-scoped localStorage — the ONE storage pathway for per-install state.
 *
 * Under the management tunnel every bot serves from the SAME origin
 * (neboai.com), so raw localStorage is shared across /t/<botA>/ and
 * /t/<botB>/: one bot's flags (tour done, cached onboarding state, theme,
 * token) leak into — or get cleared by — every other bot. Scoping keys by the
 * runtime base gives each bot its own namespace; on desktop base is "" and
 * keys are unchanged, so existing installs keep their state.
 */
import { base } from '$app/paths';

const prefix = base ? `${base}:` : '';

function ls(): Storage | null {
	return typeof localStorage === 'undefined' ? null : localStorage;
}

export const storage = {
	get(key: string): string | null {
		return ls()?.getItem(prefix + key) ?? null;
	},
	set(key: string, value: string): void {
		try {
			ls()?.setItem(prefix + key, value);
		} catch {
			/* quota/private mode — per-install state is best-effort */
		}
	},
	remove(key: string): void {
		ls()?.removeItem(prefix + key);
	}
};
