// When a turn-end event ends the run on its session. A message sent while an
// employee works is taken into the running turn, and its own stream ends at
// once with `chat_complete` carrying this typed stop: the run is still going,
// so nothing that shows it working may clear on it.

/** The typed stop of a message taken into the turn already running. */
export const QUEUED_INTO_RUNNING_TURN = 'queued_into_running_turn';

/** Whether `chat_complete` / `chat_error` / `chat_cancelled` ends the run on
 *  its session: every one does except a queued message's own completion. */
export function endsRun(data: { stop_reason?: unknown } | null | undefined): boolean {
	return data?.stop_reason !== QUEUED_INTO_RUNNING_TURN;
}
