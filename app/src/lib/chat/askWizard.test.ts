// A question card is steps the owner goes through: his answers are kept as
// he moves Back and Next, and one Submit sends them all as one reply.
import { describe, it, expect } from 'vitest';
import { chooseOther, emptyAnswer, pick, replyAnswers, replyValue, stepValue, wizardSteps, type StepAnswer } from './askWizard';

const widgets = [
	{ type: 'options' as const, header: 'Length', question: 'How long?', options: [{ label: '30 seconds', description: 'Tight' }, '60 seconds'] },
	{ type: 'options' as const, header: 'Music', question: 'Which music?', options: ['Upbeat', 'Calm', 'None'], multiSelect: true },
	{ type: 'options' as const, header: 'Title', question: 'What title?' }
];

/** The card's state as the component keeps it: where he is, and each
 *  step's answer. */
function wizard() {
	const steps = wizardSteps('How long?', widgets);
	let at = 0;
	let answers: StepAnswer[] = [];
	const cur = () => answers[at] ?? emptyAnswer();
	const set = (a: StepAnswer) => {
		answers = [...answers];
		answers[at] = a;
	};
	return {
		steps,
		get at() { return at; },
		tap: (label: string) => set(pick(steps[at], cur(), label)),
		other: (words: string) => set({ ...chooseOther(steps[at], cur()), other: words, otherOn: true }),
		next: () => (at = Math.min(at + 1, steps.length - 1)),
		back: () => (at = Math.max(at - 1, 0)),
		value: () => stepValue(steps[at], cur()),
		submit: () => replyValue(steps, answers)
	};
}

describe('ask wizard', () => {
	it('reads each step: its header, question and options, a string option a label with no description', () => {
		const steps = wizardSteps('How long?', widgets);
		expect(steps.map((s) => s.header)).toEqual(['Length', 'Music', 'Title']);
		expect(steps[0].options).toEqual([{ label: '30 seconds', description: 'Tight', recommended: undefined }, { label: '60 seconds' }]);
		expect(steps[1].multi).toBe(true);
		// A card from before the wizard: one step, its question the prompt.
		const old = wizardSteps('Which one?', [{ type: 'options', options: ['A', 'B'] }]);
		expect(old).toHaveLength(1);
		expect(old[0].question).toBe('Which one?');
	});

	it('three questions: going back keeps the answers, and Submit sends all three', () => {
		const w = wizard();
		w.tap('30 seconds');
		w.tap('60 seconds');
		expect(w.value()).toBe('60 seconds');
		w.next();
		w.tap('Upbeat');
		w.tap('Calm');
		w.next();
		w.other('Spring launch');
		w.back();
		expect(w.at).toBe(1);
		expect(w.value()).toBe('Upbeat, Calm');
		w.back();
		expect(w.value()).toBe('60 seconds');
		w.next();
		w.tap('Upbeat');
		expect(w.value()).toBe('Calm');
		w.next();
		expect(w.value()).toBe('Spring launch');
		const reply = w.submit();
		expect(JSON.parse(reply)).toEqual(['60 seconds', 'Calm', 'Spring launch']);
		expect(replyAnswers(w.steps, reply)).toEqual(['60 seconds', 'Calm', 'Spring launch']);
	});

	it('one question sends its answer as it always did; Other replaces a single pick', () => {
		const steps = wizardSteps('Which one?', [{ type: 'options', options: ['A', 'B'] }]);
		let a = pick(steps[0], emptyAnswer(), 'A');
		expect(replyValue(steps, [a])).toBe('A');
		a = { ...chooseOther(steps[0], a), other: ' my own ' };
		expect(replyValue(steps, [a])).toBe('my own');
		// His own words for all of them, typed in the chat, are the first step's.
		expect(replyAnswers(wizardSteps('', widgets), 'short and calm')).toEqual(['short and calm', '', '']);
	});
});
