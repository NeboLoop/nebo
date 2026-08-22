/**
 * Shared time formatting — the ONE implementation for message timestamps and
 * relative ("time ago") labels across all surfaces.
 */
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

/** Format a timestamp for display (e.g. "3:42 PM"). */
export function formatTime(ts: string | number): string {
	try {
		const n = typeof ts === 'number' ? ts : Number(ts);
		const date = !isNaN(n) && n > 0 ? new Date(n < 1e12 ? n * 1000 : n) : new Date(String(ts));
		if (isNaN(date.getTime())) return '';
		return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	} catch {
		return '';
	}
}

/**
 * Relative "time ago" label, i18n'd (usable outside components via `get(t)`).
 * - 'long' (default): time.justNow / minutesAgo / hoursAgo / daysAgo / monthsAgo
 * - 'short' (compact sidebar meta): time.now / minutesShort / hoursShort / daysShort
 * Both styles share the same tier logic; only the key family differs.
 */
export function formatRelative(date: string | number | Date, style: 'long' | 'short' = 'long'): string {
	const $t = get(t);
	const ms = new Date(date).getTime();
	if (isNaN(ms)) return '';
	const mins = Math.floor((Date.now() - ms) / 60_000);
	if (mins < 1) return $t(style === 'short' ? 'time.now' : 'time.justNow');
	if (mins < 60) return $t(style === 'short' ? 'time.minutesShort' : 'time.minutesAgo', { values: { n: mins } });
	const hours = Math.floor(mins / 60);
	if (hours < 24) return $t(style === 'short' ? 'time.hoursShort' : 'time.hoursAgo', { values: { n: hours } });
	const days = Math.floor(hours / 24);
	// The short family has no month tier — days is its coarsest unit.
	if (style === 'short') return $t('time.daysShort', { values: { n: days } });
	if (days < 30) return $t('time.daysAgo', { values: { n: days } });
	return $t('time.monthsAgo', { values: { n: Math.floor(days / 30) } });
}
