// The 5-second `agent_progress` snapshot is the one status path for a
// running turn: it lists every live run, and a run with no call running is
// the employee thinking about its next step. A slow model reads as
// "Thinking…" here, never as an error that ends the turn.

type RunSnapshot = { sessionKey?: unknown; currentTool?: unknown };

/** Whether the snapshot's run on this conversation is thinking: live, with no
 *  call running. False when no run on it is in the snapshot. */
export function isThinking(runs: unknown, sessionKey: string): boolean {
	if (!sessionKey || !Array.isArray(runs)) return false;
	const run = (runs as RunSnapshot[]).find((r) => r?.sessionKey === sessionKey);
	return !!run && !run.currentTool;
}

/** The live line for what the model has thought so far this stretch of the
 *  turn (the `thinking` stream): the last line with words on it, from its
 *  last sentence break on. */
export function latestThought(thought: string): string {
	const lines = thought.split('\n').map((l) => l.trim()).filter(Boolean);
	if (!lines.length) return '';
	const last = lines[lines.length - 1];
	const sentences = last.split(/(?<=[.!?])\s+/).map((x) => x.trim()).filter(Boolean);
	return sentences.length ? sentences[sentences.length - 1] : last;
}
