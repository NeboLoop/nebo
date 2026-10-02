/**
 * Chat transcript auto-scroll helpers.
 *
 * Follow new content while the user is near the bottom. Manual scroll-up
 * disables follow until they return near the bottom or send a new prompt.
 * Programmatic scrolls must not be mistaken for user scroll-up (the classic
 * smooth-scroll race that leaves auto-scroll stuck off).
 */

export const NEAR_BOTTOM_PX = 100;

export interface ScrollMetrics {
	scrollTop: number;
	scrollHeight: number;
	clientHeight: number;
}

export function distanceFromBottom(m: ScrollMetrics): number {
	return m.scrollHeight - m.scrollTop - m.clientHeight;
}

export function isNearBottom(m: ScrollMetrics, threshold = NEAR_BOTTOM_PX): boolean {
	return distanceFromBottom(m) <= threshold;
}

export function shouldShowScrollButton(
	m: ScrollMetrics,
	threshold = NEAR_BOTTOM_PX,
): boolean {
	return !isNearBottom(m, threshold);
}

/**
 * Update the auto-scroll flag from a scroll event.
 * Programmatic scrolls leave the flag unchanged.
 */
export function autoScrollAfterUserScroll(
	currentlyEnabled: boolean,
	metrics: ScrollMetrics,
	opts: { programmatic: boolean; threshold?: number },
): boolean {
	if (opts.programmatic) return currentlyEnabled;
	const near = isNearBottom(metrics, opts.threshold);
	if (currentlyEnabled && !near) return false;
	if (!currentlyEnabled && near) return true;
	return currentlyEnabled;
}

/** Whether the transcript should pin to bottom after a messages update. */
export function shouldFollowMessages(opts: {
	initialScrollDone: boolean;
	autoScrollEnabled: boolean;
	/** User just sent — always follow even if they had scrolled up. */
	forceFollow: boolean;
}): boolean {
	if (!opts.initialScrollDone) return false;
	return opts.forceFollow || opts.autoScrollEnabled;
}

export function isTrailingUserMessage(
	messages: ReadonlyArray<{ type: string }>,
): boolean {
	const last = messages[messages.length - 1];
	return last?.type === 'user';
}

/**
 * Dependency key so streaming content growth (same length, growing content)
 * retriggers follow-scroll — not only `messages.length` changes.
 */
export function messagesScrollKey(
	messages: ReadonlyArray<{
		type: string;
		content?: string;
		streaming?: boolean;
		tools?: ReadonlyArray<unknown>;
	}>,
): string {
	const last = messages[messages.length - 1];
	if (!last) return '0';
	const contentLen = last.content?.length ?? 0;
	const toolsLen = last.tools?.length ?? 0;
	const streaming = last.streaming ? '1' : '0';
	return `${messages.length}:${last.type}:${contentLen}:${toolsLen}:${streaming}`;
}

/** One transcript row's edges, in viewport pixels (getBoundingClientRect). */
export interface RowEdges {
	top: number;
	bottom: number;
}

/** The reader's place in the transcript: at its end, or the first row still
 *  in view and how far below the scroller's top edge that row starts. */
export interface ScrollAnchor {
	atBottom: boolean;
	index: number;
	offset: number;
}

/** Where the reader is, taken before the transcript column changes width
 *  (the Work pane opening or closing reflows every row). */
export function captureAnchor(m: ScrollMetrics, viewTop: number, rows: ReadonlyArray<RowEdges>): ScrollAnchor {
	if (isNearBottom(m)) return { atBottom: true, index: -1, offset: 0 };
	const index = rows.findIndex((r) => r.bottom > viewTop);
	if (index === -1) return { atBottom: true, index: -1, offset: 0 };
	return { atBottom: false, index, offset: rows[index].top - viewTop };
}

/** The scrollTop that puts the reader back where `anchor` says, once the
 *  rows have reflowed: the end stays the end, and a row in view keeps its
 *  distance from the top edge. */
export function restoredScrollTop(
	anchor: ScrollAnchor,
	m: ScrollMetrics,
	viewTop: number,
	rows: ReadonlyArray<RowEdges>,
): number {
	const max = Math.max(0, m.scrollHeight - m.clientHeight);
	if (anchor.atBottom) return max;
	const row = rows[anchor.index];
	if (!row) return Math.min(m.scrollTop, max);
	const top = m.scrollTop + (row.top - viewTop) - anchor.offset;
	return Math.max(0, Math.min(max, top));
}
