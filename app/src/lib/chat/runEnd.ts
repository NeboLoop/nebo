// When a turn-end event ends the run on its session. A message sent while an
// employee works is taken into the running turn, and its own stream ends at
// once with `chat_complete` carrying this typed stop: the run is still going,
// so nothing that shows it working may clear on it.

/** The typed stop of a message taken into the turn already running. */
export const QUEUED_INTO_RUNNING_TURN = 'queued_into_running_turn';

/** The typed stop of a turn no AI could take: the bot is not signed in to
 *  NeboAI. It arrives on `chat_error` (and its stored row) and is worded
 *  here, once, in the thread, with the Sign in action, never as an error. */
export const NOT_SIGNED_IN = 'not_signed_in';

/** The typed stop of a turn whose balance was held by work already running
 *  (the balance covers the turn): worded here, once, in the thread, in the
 *  owner's language — never as out of balance, never as an error. */
export const BALANCE_RESERVED = 'balance_reserved';

/** Whether `chat_complete` / `chat_error` / `chat_cancelled` ends the run on
 *  its session: every one does except a queued message's own completion. */
export function endsRun(data: { stop_reason?: unknown } | null | undefined): boolean {
	return data?.stop_reason !== QUEUED_INTO_RUNNING_TURN;
}
