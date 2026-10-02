// Teach-a-task: where a demonstration is recorded, and what the chat
// header's monitor icon does about it.
//
// A bot on the owner's own computer records the owner's real screen: the
// owner does the task where they always do it, with no new window and no
// virtual machine. A cloud bot (or a headless server) has no screen of its
// own and records on its virtual computer, watched in the full-window view.
// On a bot with its own screen that virtual computer is still there, behind
// Developer mode.

/** The header's computer actions for this bot. `ownScreen` is null until
 *  the bot has said; until then the header behaves as it always did. */
export function computerActions(
	ownScreen: boolean | null,
	devMode: boolean
): { teach: boolean; computer: boolean } {
	if (ownScreen === true) return { teach: true, computer: devMode };
	return { teach: false, computer: true };
}

/** Whether a recording the bot started (teach/start's `where`) is watched
 *  in the full-window computer view. Only a local recording is not. */
export function watchesComputer(where: unknown): boolean {
	return where !== 'local';
}
