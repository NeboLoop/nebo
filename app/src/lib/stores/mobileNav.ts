import { writable } from 'svelte/store';

/**
 * Mobile drawer state for the two workspace panes. On `md:` and up the panes
 * are fixed sidebars and these stores are ignored; below `md` each pane is a
 * slide-over toggled from the header (agents) or the threads bar (chats).
 * Navigation closes both (see the workspace layout $effect).
 */
export const mobileAgentsOpen = writable(false);
export const mobileChatsOpen = writable(false);
