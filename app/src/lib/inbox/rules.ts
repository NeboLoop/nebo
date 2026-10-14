// What the assistant handles for the owner: the rules he said yes to (each
// with when and where he said it, and its history), the kinds he said never
// to offer again, and the candidates he can turn into a rule himself. One
// store, loaded from the server's one list; every change goes through the
// server and the list is read again.

import { writable, get } from 'svelte/store';
import { answerVia, ASSISTANT_ID } from './item';

export interface Consent {
	/** Unix seconds. */
	at: number;
	/** The owner's zone then (IANA), its abbreviation and offset. */
	timeZone: string;
	zoneAbbr?: string;
	offsetSeconds?: number;
	via: string;
}

export interface RuleEvent extends Consent {
	event: 'turned_on' | 'edited' | 'paused' | 'resumed' | string;
	detail?: string;
}

export interface RuleShape {
	id: string;
	agentId: string;
	employee: string;
	covers: string;
	does: string;
	stillAsks: string;
	cantUndo: boolean;
	ceilingCents?: number | null;
	counterparties: string[];
}

export interface LearnedRule extends RuleShape {
	/** The bot the rule applies on. */
	bot?: string;
	paused: boolean;
	uses: number;
	lastUsedAt?: number | null;
	consent: Consent;
	history: RuleEvent[];
}

export interface NeverOffer {
	id: string;
	agentId: string;
	employee: string;
	what: string;
}

export type RuleCandidate = RuleShape;

export interface LearnedRules {
	rules: LearnedRule[];
	neverOffer: NeverOffer[];
	candidates: RuleCandidate[];
}

export const learnedRules = writable<LearnedRules | null>(null);

/** Where the rules list lives: a section of the assistant's own settings. */
export const rulesPath = (): string => `/${ASSISTANT_ID}/settings/handles`;

export async function loadLearnedRules(): Promise<LearnedRules> {
	const api = await import('$lib/api/nebo');
	const res = (await api.listLearnedRules()) as unknown as Partial<LearnedRules>;
	const rules = { rules: res.rules ?? [], neverOffer: res.neverOffer ?? [], candidates: res.candidates ?? [] };
	learnedRules.set(rules);
	return rules;
}

/** The rule with `id`, loading the list once when it isn't here yet. */
export async function ruleById(id: string): Promise<LearnedRule | null> {
	const have = get(learnedRules) ?? (await loadLearnedRules());
	return have.rules.find((r) => r.id === id) ?? null;
}

/** Stop or restart a rule. Nothing is deleted: a paused rule keeps its history. */
export async function setRulePaused(id: string, paused: boolean): Promise<void> {
	const api = await import('$lib/api/nebo');
	const via = answerVia();
	await (paused ? api.pauseLearnedRule(id, { via }) : api.resumeLearnedRule(id, { via }));
	await loadLearnedRules();
}

/** Change a rule's ceiling (in cents; null = no ceiling). */
export async function editRule(id: string, ceilingCents: number | null): Promise<void> {
	const api = await import('$lib/api/nebo');
	await api.updateLearnedRule(id, { ceilingCents, via: answerVia() });
	await loadLearnedRules();
}

/** Turn one of the owner's own recent answers into a rule. */
export async function addRule(candidateId: string, ceilingCents: number | null): Promise<void> {
	const api = await import('$lib/api/nebo');
	await api.addLearnedRule({ candidateId, ceilingCents, via: answerVia() });
	await loadLearnedRules();
}

/** Take a kind off the "Never offer again" list. */
export async function offerAgain(id: string): Promise<void> {
	const api = await import('$lib/api/nebo');
	await api.offerAgain(id);
	await loadLearnedRules();
}

/** A moment on the owner's clock then: "Wed, Oct 14, 2026, 9:02 AM MDT".
 *  The clock is the offset the server recorded with it, so the words never
 *  depend on this browser's zone. */
export function formatMoment(c: Pick<Consent, 'at' | 'timeZone' | 'zoneAbbr' | 'offsetSeconds'>, locale?: string): string {
	const shifted = new Date((c.at + (c.offsetSeconds ?? 0)) * 1000);
	const when = new Intl.DateTimeFormat(locale, {
		weekday: 'short',
		month: 'short',
		day: 'numeric',
		year: 'numeric',
		hour: 'numeric',
		minute: '2-digit',
		timeZone: 'UTC'
	}).format(shifted);
	return `${when} ${c.zoneAbbr || c.timeZone}`.trim();
}

/** Dollars from cents, for the ceiling field. */
export const dollars = (cents?: number | null): string => (cents == null ? '' : String(cents / 100));
/** Cents from what the owner typed; empty is no ceiling. */
export function cents(text: string): number | null {
	const n = Number(text.replace(/[$,\s]/g, ''));
	return text.trim() === '' || !Number.isFinite(n) || n < 0 ? null : Math.round(n * 100);
}
