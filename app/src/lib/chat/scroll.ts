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
