// An employee's question card, as steps the owner goes through: one step per
// question (`ask_owner`'s `questions`), each with its header, its choices (a
// short label and a one-line description) and "Other" for his own words. A
// single question is a card of one step. The owner moves Back and Next with
// his answers kept, and one Submit sends them all: one question's answer as
// it always went (a label, labels joined by ", ", or his words); several as
// a JSON list, one answer per question, in order (`ask_owner_tool.rs`
// `answers`).

import type { AskOption, AskWidgetDef } from '$lib/components/chat/AskWidget.svelte';

export interface WizardOption {
	label: string;
	description?: string;
	recommended?: boolean;
}

export interface WizardStep {
	/** The few words on its step chip; empty on a single question. */
	header: string;
	/** The question, as the employee wrote it. */
	question: string;
	options: WizardOption[];
	multi: boolean;
	/** One pick from a long list: a dropdown. */
	select: boolean;
	/** The widget it came from: its words keys, its context. */
	widget: AskWidgetDef;
}

/** What the owner has given on one step so far. */
export interface StepAnswer {
	picked: string[];
	/** "Other" is chosen: his own words answer it. */
	otherOn: boolean;
	other: string;
}

export function normalizeOptions(options: AskOption[] | undefined): WizardOption[] {
	return (options ?? []).map((o) =>
		typeof o === 'string' ? { label: o } : { label: o.label, description: o.description, recommended: o.recommended }
	);
}

/** The card's steps. A question card from before the wizard carries its
 *  question as the ask's prompt; each step carries its own. */
export function wizardSteps(prompt: string, widgets: AskWidgetDef[] | undefined): WizardStep[] {
	return (widgets ?? []).map((w, i) => ({
		header: (w as { header?: string }).header ?? '',
		question: (w as { question?: string }).question ?? (i === 0 ? prompt : ''),
		options: normalizeOptions(w.options),
		multi: w.multiSelect === true || w.type === 'checkbox',
		select: w.style === 'select' && !(w.multiSelect === true),
		widget: w
	}));
}

export function emptyAnswer(): StepAnswer {
	return { picked: [], otherOn: false, other: '' };
}

/** Tap on an option: a single pick replaces the one before it (and turns
 *  "Other" off); on a multi-select it toggles. */
export function pick(step: WizardStep, a: StepAnswer, label: string): StepAnswer {
	if (!step.multi) return { picked: [label], otherOn: false, other: a.other };
	const picked = a.picked.includes(label) ? a.picked.filter((p) => p !== label) : [...a.picked, label];
	return { ...a, picked };
}

/** Tap on "Other": his own words answer it. On a single pick it replaces
 *  the option; on a multi-select it goes beside the ones picked. */
export function chooseOther(step: WizardStep, a: StepAnswer): StepAnswer {
	return step.multi ? { ...a, otherOn: !a.otherOn } : { picked: [], otherOn: true, other: a.other };
}

/** The step's answer as it is sent; empty when it has none yet. */
export function stepValue(step: WizardStep, a: StepAnswer): string {
	const own = a.otherOn ? a.other.trim() : '';
	if (step.multi) return [...step.options.map((o) => o.label).filter((l) => a.picked.includes(l)), ...(own ? [own] : [])].join(', ');
	return a.otherOn ? own : (a.picked[0] ?? '');
}

/** The one reply the card sends: one question's answer as it is; several
 *  as a JSON list of answers, in order. */
export function replyValue(steps: WizardStep[], answers: StepAnswer[]): string {
	const values = steps.map((s, i) => stepValue(s, answers[i] ?? emptyAnswer()));
	return steps.length === 1 ? values[0] : JSON.stringify(values);
}

/** The answer to each step a sent reply holds, for its receipt. A reply
 *  that is not one answer per step (his own words for all of them, typed in
 *  the chat) is the first step's. */
export function replyAnswers(steps: WizardStep[], response: string): string[] {
	if (steps.length > 1) {
		try {
			const list = JSON.parse(response);
			if (Array.isArray(list) && list.length === steps.length && list.every((v) => typeof v === 'string')) return list;
		} catch {
			// his own words, below
		}
	}
	return [response, ...steps.slice(1).map(() => '')];
}
