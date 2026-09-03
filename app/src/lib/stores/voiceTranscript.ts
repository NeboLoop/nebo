export type TranscriptEntry = { speaker: 'user' | 'agent'; text: string };

/**
 * The finished transcript of an utterance. The realtime engine sends it
 * cumulatively: a late correction arrives as a second, fuller finish for the
 * same utterance. While no agent turn has started, that finish replaces the
 * entry the first one opened instead of adding a twin (the doubled "You"
 * bubbles in the voice overlay, 2026-09-03).
 */
export function finishUserTranscript(
	transcripts: TranscriptEntry[],
	text: string,
	userEntryOpen: boolean
): TranscriptEntry[] {
	if (!text) return transcripts;
	const last = transcripts[transcripts.length - 1];
	if (userEntryOpen && last?.speaker === 'user') {
		return [...transcripts.slice(0, -1), { speaker: 'user', text }];
	}
	return [...transcripts, { speaker: 'user', text }];
}
