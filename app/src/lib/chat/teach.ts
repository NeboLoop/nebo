// Teach-a-task: ONE action, behind the chat header's one monitor icon and
// the composer's Teach a task entry alike.
//
// Where it records is the bot's to say, from its real platform: a bot on a
// Mac, Windows or Linux desktop host records the owner's own screen, right
// where they work, with no window and no virtual machine; a cloud bot or a
// headless server has no screen and records on its computer, watched in the
// full-window view. The app never guesses: it asks the bot to start, and
// the bot's answer (`where`) says which it did.

/** Whether a recording the bot started (teach/start's `where`) is watched in
 *  the full-window computer view. Only a recording of the host's own screen
 *  is not. */
export function watchesComputer(where: unknown): boolean {
	return where !== 'local';
}

/** Whether the one icon offers a choice (Teach a task, or just open the
 *  computer) instead of starting a recording at once. A cloud or headless
 *  bot always does: opening its computer to look at it must never record.
 *  A host bot does in Developer mode, where its virtual computer is still
 *  reachable. A host bot otherwise, or one not yet known, gets the one
 *  action. */
export function offersComputer(ownScreen: boolean | null, devMode: boolean): boolean {
	return ownScreen === false || (ownScreen === true && devMode);
}

/** The settings pane that grants Screen Recording on macOS. */
export const SCREEN_RECORDING_SETTINGS =
	'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture';

/** What a failed start means for the owner: the message, and whether it is
 *  the missing Screen Recording permission (which gets the settings button). */
export function teachFailure(e: unknown): { message: string; needsScreenPermission: boolean } {
	const data = (e as { response?: { data?: { reason?: unknown } } })?.response?.data;
	return {
		message: e instanceof Error ? e.message : String(e),
		needsScreenPermission: data?.reason === 'screen_recording_permission'
	};
}
