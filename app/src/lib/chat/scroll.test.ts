import { describe, expect, it } from 'vitest';
import {
	NEAR_BOTTOM_PX,
	captureAnchor,
	restoredScrollTop,
	autoScrollAfterUserScroll,
	distanceFromBottom,
	isNearBottom,
	isTrailingUserMessage,
	messagesScrollKey,
	shouldFollowMessages,
	shouldShowScrollButton,
	type ScrollMetrics,
} from './scroll';

function metrics(partial: Partial<ScrollMetrics> & Pick<ScrollMetrics, 'scrollTop'>): ScrollMetrics {
	return {
		scrollHeight: 1000,
		clientHeight: 400,
		...partial,
	};
}

describe('distanceFromBottom / isNearBottom', () => {
	it('reports zero when pinned to the bottom', () => {
		const m = metrics({ scrollTop: 600 }); // 1000 - 600 - 400 = 0
		expect(distanceFromBottom(m)).toBe(0);
		expect(isNearBottom(m)).toBe(true);
	});

	it('treats within-threshold as near bottom', () => {
		const m = metrics({ scrollTop: 600 - NEAR_BOTTOM_PX });
		expect(distanceFromBottom(m)).toBe(NEAR_BOTTOM_PX);
		expect(isNearBottom(m)).toBe(true);
	});

	it('treats past-threshold as not near bottom', () => {
		const m = metrics({ scrollTop: 600 - NEAR_BOTTOM_PX - 1 });
		expect(isNearBottom(m)).toBe(false);
		expect(shouldShowScrollButton(m)).toBe(true);
	});
});

describe('autoScrollAfterUserScroll', () => {
	it('disables follow when the user scrolls up past the threshold', () => {
		const m = metrics({ scrollTop: 100 });
		expect(autoScrollAfterUserScroll(true, m, { programmatic: false })).toBe(false);
	});

	it('re-enables follow when the user scrolls back near the bottom', () => {
		const m = metrics({ scrollTop: 600 });
		expect(autoScrollAfterUserScroll(false, m, { programmatic: false })).toBe(true);
	});

	it('does not disable follow during a programmatic scroll (smooth-scroll race)', () => {
		// Mid smooth-scroll: still far from the final bottom, but we initiated it.
		const m = metrics({ scrollTop: 100 });
		expect(autoScrollAfterUserScroll(true, m, { programmatic: true })).toBe(true);
	});

	it('does not re-enable follow during a programmatic scroll away from bottom', () => {
		const m = metrics({ scrollTop: 100 });
		expect(autoScrollAfterUserScroll(false, m, { programmatic: true })).toBe(false);
	});

	it('leaves the flag unchanged when still near bottom', () => {
		const m = metrics({ scrollTop: 580 });
		expect(autoScrollAfterUserScroll(true, m, { programmatic: false })).toBe(true);
	});
});

describe('shouldFollowMessages', () => {
	it('does not follow before the initial pin finishes', () => {
		expect(
			shouldFollowMessages({
				initialScrollDone: false,
				autoScrollEnabled: true,
				forceFollow: true,
			}),
		).toBe(false);
	});

	it('follows when auto-scroll is enabled', () => {
		expect(
			shouldFollowMessages({
				initialScrollDone: true,
				autoScrollEnabled: true,
				forceFollow: false,
			}),
		).toBe(true);
	});

	it('does not follow when the user has scrolled up', () => {
		expect(
			shouldFollowMessages({
				initialScrollDone: true,
				autoScrollEnabled: false,
				forceFollow: false,
			}),
		).toBe(false);
	});

	it('force-follows on send even if the user had scrolled up', () => {
		expect(
			shouldFollowMessages({
				initialScrollDone: true,
				autoScrollEnabled: false,
				forceFollow: true,
			}),
		).toBe(true);
	});
});

describe('isTrailingUserMessage', () => {
	it('is true when the latest message is from the user (prompt just entered)', () => {
		expect(
			isTrailingUserMessage([
				{ type: 'assistant' },
				{ type: 'user' },
			]),
		).toBe(true);
	});

	it('is false while the assistant is streaming', () => {
		expect(
			isTrailingUserMessage([
				{ type: 'user' },
				{ type: 'assistant' },
			]),
		).toBe(false);
	});

	it('is false for an empty transcript', () => {
		expect(isTrailingUserMessage([])).toBe(false);
	});
});

describe('messagesScrollKey', () => {
	it('changes when message length changes', () => {
		const a = messagesScrollKey([{ type: 'user', content: 'hi' }]);
		const b = messagesScrollKey([
			{ type: 'user', content: 'hi' },
			{ type: 'assistant', content: '', streaming: true },
		]);
		expect(a).not.toBe(b);
	});

	it('changes when streaming content grows without a new message', () => {
		const before = messagesScrollKey([
			{ type: 'user', content: 'hi' },
			{ type: 'assistant', content: 'Hel', streaming: true },
		]);
		const after = messagesScrollKey([
			{ type: 'user', content: 'hi' },
			{ type: 'assistant', content: 'Hello world', streaming: true },
		]);
		expect(before).not.toBe(after);
	});

	it('changes when tools are attached to the open reply', () => {
		const before = messagesScrollKey([
			{ type: 'assistant', content: '…', streaming: true, tools: [] },
		]);
		const after = messagesScrollKey([
			{
				type: 'assistant',
				content: '…',
				streaming: true,
				tools: [{ name: 'web' }],
			},
		]);
		expect(before).not.toBe(after);
	});

	it('is stable when nothing about the tail changed', () => {
		const msgs = [
			{ type: 'user' as const, content: 'hi' },
			{ type: 'assistant' as const, content: 'yo', streaming: false },
		];
		expect(messagesScrollKey(msgs)).toBe(messagesScrollKey([...msgs]));
	});
});

describe('send-then-stream follow scenario', () => {
	it('stays following from scrolled-up → send → stream growth', () => {
		// User had scrolled up
		let enabled = autoScrollAfterUserScroll(
			true,
			metrics({ scrollTop: 50 }),
			{ programmatic: false },
		);
		expect(enabled).toBe(false);

		const msgsAfterSend = [
			{ type: 'assistant', content: 'old' },
			{ type: 'user', content: 'new prompt' },
		];
		expect(
			shouldFollowMessages({
				initialScrollDone: true,
				autoScrollEnabled: enabled,
				forceFollow: isTrailingUserMessage(msgsAfterSend),
			}),
		).toBe(true);

		// Send re-enables follow (ChatPane does this when forceFollow)
		enabled = true;

		const streaming = [
			...msgsAfterSend,
			{ type: 'assistant', content: 'partial', streaming: true },
		];
		expect(
			shouldFollowMessages({
				initialScrollDone: true,
				autoScrollEnabled: enabled,
				forceFollow: isTrailingUserMessage(streaming),
			}),
		).toBe(true);

		// Mid programmatic pin must not kill follow
		enabled = autoScrollAfterUserScroll(
			enabled,
			metrics({ scrollTop: 200 }),
			{ programmatic: true },
		);
		expect(enabled).toBe(true);

		const grown = [
			...msgsAfterSend,
			{ type: 'assistant', content: 'partial answer that grew a lot', streaming: true },
		];
		expect(messagesScrollKey(streaming)).not.toBe(messagesScrollKey(grown));
		expect(
			shouldFollowMessages({
				initialScrollDone: true,
				autoScrollEnabled: enabled,
				forceFollow: false,
			}),
		).toBe(true);
	});
});

describe('captureAnchor / restoredScrollTop: closing the file viewer returns to the same place', () => {
	// Rows are laid out top to bottom; `viewTop` is the scroller's top edge.
	const layout = (heights: number[], scrollTop: number, viewTop = 50) => {
		let y = viewTop - scrollTop;
		return heights.map((h) => {
			const r = { top: y, bottom: y + h };
			y += h;
			return r;
		});
	};

	it('keeps the row being read at the same distance from the top across a reflow', () => {
		// Narrow column (pane open): taller rows. The reader is mid-transcript.
		const narrow = [400, 600, 300, 500, 400];
		const before = { scrollTop: 700, scrollHeight: 2200, clientHeight: 600 };
		const anchor = captureAnchor(before, 50, layout(narrow, 700));
		expect(anchor.atBottom).toBe(false);
		expect(anchor.index).toBe(1); // row 1 spans 400–1000, in view at 700
		expect(anchor.offset).toBe(-300);

		// Pane closed: the column widens and every row gets shorter. With the
		// old scrollTop the view would land past the rows being read.
		const wide = [200, 300, 150, 250, 200];
		const after = { scrollTop: 500, scrollHeight: 1100, clientHeight: 600 }; // clamped
		const top = restoredScrollTop(anchor, after, 50, layout(wide, 500));
		// Row 1 starts at 200 in the wide layout; 300px of it was above the edge.
		expect(top).toBe(500);
		expect(layout(wide, top)[1].top - 50).toBe(-300);
	});

	it('the end of the conversation stays the end', () => {
		const before = { scrollTop: 1600, scrollHeight: 2200, clientHeight: 600 };
		const anchor = captureAnchor(before, 50, layout([2200], 1600));
		expect(anchor.atBottom).toBe(true);
		const after = { scrollTop: 0, scrollHeight: 1100, clientHeight: 600 };
		expect(restoredScrollTop(anchor, after, 50, layout([1100], 0))).toBe(500);
	});

	it('never scrolls past the rows into empty space', () => {
		const anchor = { atBottom: false, index: 9, offset: 0 };
		const m = { scrollTop: 4000, scrollHeight: 1100, clientHeight: 600 };
		expect(restoredScrollTop(anchor, m, 50, layout([1100], 0))).toBe(500);
	});
});
