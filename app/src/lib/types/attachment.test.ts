import { describe, expect, it } from 'vitest';
import { firstFrame } from './attachment';

describe('firstFrame', () => {
	it('asks the player for the first frame', () => {
		expect(firstFrame('/api/v1/files/cut.mp4')).toBe('/api/v1/files/cut.mp4#t=0.001');
	});
	it('keeps a fragment already given', () => {
		expect(firstFrame('/api/v1/files/cut.mp4#t=3')).toBe('/api/v1/files/cut.mp4#t=3');
	});
});
