import { describe, expect, it } from 'vitest';
import { beginSave, endSave, hasPendingSave, isOwnVersion, setPendingSave, viewerKey } from './pending';

describe('sheet saves and the panel', () => {
	it('tracks unsaved edits per document', () => {
		setPendingSave('d1', async () => {});
		expect(hasPendingSave('d1')).toBe(true);
		setPendingSave('d1', null);
		expect(hasPendingSave('d1')).toBe(false);
	});

	it('keeps the viewer mounted for a version it saved itself', () => {
		const k3 = viewerKey('', 'doc', 3, 'false');
		expect(k3).toBe('doc:3:false');
		// The save is in flight when the new version is announced.
		beginSave('doc');
		expect(isOwnVersion('doc', 4)).toBe(true);
		expect(viewerKey(k3, 'doc', 4, 'false')).toBe(k3);
		endSave('doc', 4);
		// After the response it is still ours.
		expect(viewerKey(k3, 'doc', 4, 'false')).toBe(k3);
		// Someone else's version remounts.
		expect(viewerKey(k3, 'doc', 5, 'false')).toBe('doc:5:false');
		// Switching to source view or another document remounts.
		expect(viewerKey(k3, 'doc', 4, 'true')).toBe('doc:4:true');
		expect(viewerKey(k3, 'other', 4, 'false')).toBe('other:4:false');
	});
});
