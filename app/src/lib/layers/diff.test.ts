import { describe, it, expect } from 'vitest';
import { parseChangeText, countChanges } from './diff';

describe('parseChangeText — unified diff', () => {
	const unified = [
		'diff --git a/rules/dunning-cadence.md b/rules/dunning-cadence.md',
		'index 1111111..2222222 100644',
		'--- a/rules/dunning-cadence.md',
		'+++ b/rules/dunning-cadence.md',
		'@@ -1,4 +1,4 @@',
		' # Dunning cadence',
		'-Chase at day 30.',
		'+Chase at day 21.',
		' Escalate after two tries.',
		''
	].join('\n');

	it('names the file by its typed folder and reads + and - as added and removed', () => {
		const set = parseChangeText(unified);
		expect(set.unified).toBe(true);
		expect(set.sections).toHaveLength(1);
		const s = set.sections[0];
		expect(s.group).toBe('rules');
		expect(s.title).toBe('dunning-cadence.md');
		expect(s.kind).toBe('changed');
		expect(s.lines.map((l) => l.kind)).toEqual(['meta', 'context', 'del', 'add', 'context']);
		expect(s.lines[2].text).toBe('Chase at day 30.');
		expect(s.lines[3].text).toBe('Chase at day 21.');
		expect(countChanges(set)).toEqual({ added: 1, removed: 1 });
	});

	it('reads a new file as added and a deleted one as removed', () => {
		const added = parseChangeText(
			['diff --git a/laws/retention.md b/laws/retention.md', 'new file mode 100644', '--- /dev/null', '+++ b/laws/retention.md', '@@ -0,0 +1 @@', '+Keep records seven years.'].join('\n')
		);
		expect(added.sections[0].kind).toBe('added');
		const removed = parseChangeText(
			['diff --git a/laws/old.md b/laws/old.md', 'deleted file mode 100644', '--- a/laws/old.md', '+++ /dev/null', '@@ -1 +0,0 @@', '-Gone.'].join('\n')
		);
		expect(removed.sections[0].kind).toBe('removed');
	});

	// `napp::pack::unified_diff` writes exactly this: one `--- a/x` / `+++ b/x`
	// pair per changed file, three lines of context, and no `diff --git` line.
	it('splits the pack loader\'s own output, which carries no git header', () => {
		const set = parseChangeText(
			[
				'--- a/rules/dunning-cadence.md',
				'+++ b/rules/dunning-cadence.md',
				'@@ -1,3 +1,3 @@',
				' # Dunning cadence',
				'-Chase at day 30.',
				'+Chase at day 21.',
				'--- /dev/null',
				'+++ b/laws/retention.md',
				'@@ -0,0 +1,1 @@',
				'+Keep records seven years.',
				'--- a/reference/fax.md',
				'+++ /dev/null',
				'@@ -1,1 +0,0 @@',
				'-Fax numbers.',
				''
			].join('\n')
		);
		expect(set.sections.map((s) => s.title)).toEqual([
			'dunning-cadence.md',
			'retention.md',
			'fax.md'
		]);
		expect(set.sections.map((s) => s.group)).toEqual(['rules', 'laws', 'reference']);
		expect(set.sections.map((s) => s.kind)).toEqual(['changed', 'added', 'removed']);
		expect(countChanges(set)).toEqual({ added: 2, removed: 2 });
	});

	it('keeps one section per file', () => {
		const two = parseChangeText(
			[
				'diff --git a/rules/a.md b/rules/a.md',
				'--- a/rules/a.md',
				'+++ b/rules/a.md',
				'@@ -1 +1 @@',
				'-one',
				'+two',
				'diff --git a/parties/b.md b/parties/b.md',
				'--- a/parties/b.md',
				'+++ b/parties/b.md',
				'@@ -1 +1 @@',
				'-three',
				'+four'
			].join('\n')
		);
		expect(two.sections.map((s) => s.title)).toEqual(['a.md', 'b.md']);
		expect(two.sections.map((s) => s.group)).toEqual(['rules', 'parties']);
	});
});

describe('parseChangeText — the pack loader section notation', () => {
	const sectioned = [
		'## Acme Plumbing (changed)',
		'',
		'We answer every call.',
		'',
		'## rules',
		'',
		'### Dunning cadence (changed)',
		'',
		'Chase at day 21.',
		'',
		'### Weekend callout (added)',
		'',
		'Double time after noon Saturday.',
		'',
		'### Paper invoices (removed)',
		''
	].join('\n');

	it('reads the marker body, the folder as a group, and each entry once', () => {
		const set = parseChangeText(sectioned);
		expect(set.unified).toBe(false);
		expect(set.sections.map((s) => s.title)).toEqual([
			'Acme Plumbing',
			'Dunning cadence',
			'Weekend callout',
			'Paper invoices'
		]);
		expect(set.sections.map((s) => s.group)).toEqual([undefined, 'rules', 'rules', 'rules']);
		expect(set.sections.map((s) => s.kind)).toEqual(['changed', 'changed', 'added', 'removed']);
	});

	it('does not invent removals: a changed entry arrives as context, an added one as added', () => {
		const set = parseChangeText(sectioned);
		const changed = set.sections[1];
		expect(changed.lines.every((l) => l.kind === 'context')).toBe(true);
		const added = set.sections[2];
		expect(added.lines.some((l) => l.kind === 'add')).toBe(true);
		expect(countChanges(set).removed).toBe(0);
	});
});

describe('parseChangeText — nothing to read', () => {
	it('is empty for empty text', () => {
		expect(parseChangeText('')).toMatchObject({ empty: true, sections: [] });
		expect(parseChangeText('   \n\n')).toMatchObject({ empty: true });
	});

	it('keeps unreadable text as one untitled section of context', () => {
		const set = parseChangeText('the pack changed, and nobody said how');
		expect(set.sections).toHaveLength(1);
		expect(set.sections[0].title).toBe('');
		expect(set.sections[0].lines[0].kind).toBe('context');
	});
});

describe('parseChangeText — what the pending entry says happened', () => {
	// A pack that has just arrived carries its whole text (`Pack::prompt_text`),
	// not a diff: `# Name (industry layer, version 1)`, `## Rules`, `### Rule`.
	const wholePack = [
		'# Acme Plumbing (company layer, version 2)',
		'',
		'We answer every call.',
		'',
		'## Rules',
		'',
		'### Deductible (always)',
		'',
		'Collect it before the van rolls.',
		''
	].join('\n');

	it('reads a whole new pack as added, folders and all', () => {
		const set = parseChangeText(wholePack, 'added');
		expect(set.sections.map((s) => s.title)).toEqual([
			'Acme Plumbing (company layer, version 2)',
			'Deductible (always)'
		]);
		expect(set.sections.map((s) => s.group)).toEqual([undefined, 'Rules']);
		expect(set.sections.every((s) => s.kind === 'added')).toBe(true);
		expect(countChanges(set).added).toBeGreaterThan(0);
		expect(countChanges(set).removed).toBe(0);
	});

	it('reads a retirement note as removed', () => {
		const set = parseChangeText('The industry layer `plumbing` was removed. Drop what came only from it.', 'removed');
		expect(set.sections[0].kind).toBe('removed');
		expect(set.sections[0].lines[0].kind).toBe('del');
	});

	it('leaves a unified diff alone: it already says what happened', () => {
		const set = parseChangeText(
			['--- a/rules/a.md', '+++ b/rules/a.md', '@@ -1 +1 @@', '-one', '+two'].join('\n'),
			'added'
		);
		expect(set.sections[0].kind).toBe('changed');
		expect(countChanges(set)).toEqual({ added: 1, removed: 1 });
	});
});
