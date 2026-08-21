import { writable } from 'svelte/store';

/// The bot's computer is INSTALLATION-wide — one desktop per Nebo, shared by
/// every employee in it — so its panel is a global drawer opened from the top
/// bar (or by teach-a-task), never per-chat state.
export const computerOpen = writable(false);
