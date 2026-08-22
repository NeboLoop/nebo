import { writable } from 'svelte/store';

/**
 * Open state for the ⌘K palette. It lives in a store rather than the root
 * layout because the affordance that opens it moved into the workspace search
 * row, and ⌘K itself is still handled globally in the root layout.
 */
export const commandPaletteOpen = writable(false);
