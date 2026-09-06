import { writable } from 'svelte/store';

/** Latest OAuth URL from `plugin_auth_url` — shown as a clickable fallback when popups are blocked. */
export const pendingPluginAuthUrl = writable<string | null>(null);
