import { describe, it, expect } from 'vitest';
import { worksOnChat } from './openOnWork';

describe('worksOnChat', () => {
	it('is the employee writing this chat’s record', () => {
		const ev = { appId: 'studio', keys: ['chat:c1:design'], source: 'employee' };
		expect(worksOnChat(ev, 'studio', 'c1')).toBe(true);
	});

	it('is not another chat, another app, the page, or an app-wide key', () => {
		expect(worksOnChat({ appId: 'studio', keys: ['chat:c2:design'], source: 'employee' }, 'studio', 'c1')).toBe(false);
		expect(worksOnChat({ appId: 'studio', keys: ['chat:c10:design'], source: 'employee' }, 'studio', 'c1')).toBe(false);
		expect(worksOnChat({ appId: 'crm', keys: ['chat:c1:design'], source: 'employee' }, 'studio', 'c1')).toBe(false);
		expect(worksOnChat({ appId: 'studio', keys: ['chat:c1:design'], source: 'page' }, 'studio', 'c1')).toBe(false);
		expect(worksOnChat({ appId: 'studio', keys: ['settings'], source: 'employee' }, 'studio', 'c1')).toBe(false);
		expect(worksOnChat(null, 'studio', 'c1')).toBe(false);
		expect(worksOnChat({ appId: 'studio', keys: ['chat:c1:design'], source: 'employee' }, 'studio', '')).toBe(false);
	});
});
