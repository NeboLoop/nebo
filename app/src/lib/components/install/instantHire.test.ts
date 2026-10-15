// A hire never waits on its setup. The install modal's Done is never held
// for the dependency cascade (the employee's roster row shows that
// progress), and the builder's Create is never held for the job being
// worked out (an employee created first gets its job offered on its page).
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const read = (rel: string) => readFileSync(join(__dirname, rel), 'utf8');

describe('a hire never waits on its setup', () => {
	it("holds the install modal's Done for nothing", () => {
		const modal = read('InstallFlowModal.svelte');
		const done = modal.split('\n').filter((line) => line.includes("{$t('common.done')}</button>"));
		expect(done.length).toBeGreaterThan(0);
		for (const button of done) expect(button).not.toContain('disabled');
	});

	it("holds the builder's Create only for a name and a request in flight", () => {
		const builder = read('../NewEmployeeModal.svelte');
		const create = builder.split('\n').find((line) => line.includes('onclick={create}')) ?? '';
		expect(create).toContain('disabled={!valid || busy}');
		expect(create).not.toContain('working');
		expect(builder).not.toMatch(/if \(!valid \|\| busy \|\| working\) return;/);
	});
});
