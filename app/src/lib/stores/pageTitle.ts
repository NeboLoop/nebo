import { writable } from 'svelte/store';
import type { Snippet } from 'svelte';

export const pageTitle = writable<string>('');
export const pageActions = writable<Snippet | null>(null);
