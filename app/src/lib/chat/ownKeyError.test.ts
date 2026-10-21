import { describe, expect, it } from 'vitest';
import { ownKeyError } from './ownKeyError';

describe('ownKeyError', () => {
	it('reads the code and provider of an own-key error', () => {
		expect(ownKeyError('neboai_signed_out(Anthropic): Sign in to NeboAI to use your own Anthropic key.')).toEqual({
			key: 'ownKey.signedOut',
			provider: 'Anthropic',
		});
		expect(ownKeyError('PROVIDER_KEY_UNUSABLE(DeepSeek): Your DeepSeek API key can\'t run this model.')?.key).toBe('ownKey.keyUnusable');
		expect(ownKeyError('PROVIDER_KEY_REJECTED(xAI): Your xAI API key was refused.')?.provider).toBe('xAI');
	});

	it('leaves every other text alone', () => {
		expect(ownKeyError('USAGE_LIMIT_EXCEEDED: no balance')).toBeNull();
		expect(ownKeyError('something(else): text')).toBeNull();
		expect(ownKeyError('')).toBeNull();
		expect(ownKeyError(undefined)).toBeNull();
	});
});
