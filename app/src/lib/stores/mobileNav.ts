import { writable } from 'svelte/store';

/**
 * Mobile drawer state for the workspace list. On `md:` and up it is a fixed
 * sidebar and this store is ignored; below `md` it is a slide-over opened from
 * the chat header's hamburger. Navigation closes it (workspace layout $effect).
 *
 * There is deliberately only ONE drawer store: the old second drawer
 * (mobileChatsOpen) died when the chats column merged into the workspace list,
 * and the work pane is URL-driven so it needs no store at any breakpoint.
 */
export const mobileAgentsOpen = writable(false);
