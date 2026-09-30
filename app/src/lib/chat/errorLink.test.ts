import { describe, expect, it } from 'vitest';
import { trailingLink } from './errorLink';

describe('trailingLink', () => {
	it('turns the label a blocked request ends on into its link', () => {
		const error =
			"This request couldn't be completed. Something earlier in this conversation may be what's blocked. " +
			'Running /compact often helps. Learn more: https://neboai.com/help/blocked-requests';
		expect(trailingLink(error)).toEqual({
			text:
				"This request couldn't be completed. Something earlier in this conversation may be what's blocked. " +
				'Running /compact often helps.',
			label: 'Learn more',
			url: 'https://neboai.com/help/blocked-requests'
		});
	});

	it('leaves an error with no link alone', () => {
		expect(trailingLink('Could not connect to nebo-1-flash. Try again.')).toBeNull();
	});
});
