import { describe, expect, it } from 'vitest';
import { answerKept, cleanCode, CODE_ENTERED, isSignInCard, signInStatus, SIGNED_IN } from './signIn';

const card = [{ type: 'sign_in', tool: 'GitHub CLI', url: 'https://github.com/login/device', code: '1A2B-3C4D', input: false }];

describe('the sign-in card', () => {
	it('never shows the code: any answer but a cancel, a finish or a failure reads as entered', () => {
		expect(signInStatus('4/0AfakeOwnerCode-XYZ')).toEqual({ state: 'entered' });
		expect(signInStatus(CODE_ENTERED)).toEqual({ state: 'entered' });
		expect(signInStatus('__skip__')).toEqual({ state: 'cancelled' });
		expect(signInStatus(SIGNED_IN)).toEqual({ state: 'signedIn' });
		expect(signInStatus('failed:The sign-in timed out.')).toEqual({ state: 'failed', reason: 'The sign-in timed out.' });
	});

	it('keeps that the code was entered, never the code, once answered here', () => {
		expect(answerKept(card, '4/0AfakeOwnerCode-XYZ')).toBe(CODE_ENTERED);
		expect(answerKept(card, '__skip__')).toBe('__skip__');
		expect(answerKept([{ type: 'options' }], 'Yes')).toBe('Yes');
		expect(isSignInCard(card)).toBe(true);
		expect(isSignInCard(undefined)).toBe(false);
	});

	it('sends the first line of what was pasted, trimmed', () => {
		expect(cleanCode('  4/0Abc-x_9 \nrm -rf /\n')).toBe('4/0Abc-x_9');
		expect(cleanCode('\n\n  code-2\n')).toBe('code-2');
		expect(cleanCode(' \n ')).toBe('');
	});
});
