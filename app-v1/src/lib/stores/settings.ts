import { writable } from 'svelte/store';

/** Path to return to when closing settings. Defaults to '/' (dashboard). */
export const settingsReturnPath = writable<string>('/');
