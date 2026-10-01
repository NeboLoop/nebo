import { writable } from 'svelte/store';

/**
 * Publish asked for from outside an app's chat (the desktop app window's
 * native menu): the id of the app whose chat starts the guided publish as
 * soon as it is open. The chat clears it when it sends the starter.
 */
export const publishRequest = writable<string | null>(null);
