// Human labels for typed interface operations, for the approval modal
// (ApprovalGate). The Permissions pages get their sentences from the server.
// Non-technical rule: an owner should never have to read "ledger.billpayment.create".

import { get } from 'svelte/store';
import { t } from 'svelte-i18n';

// The verbs and nouns live in the locale files, the whole phrase as ONE
// message (components.approvalGate.operation) so a language can order verb
// and object its own way: the action selects the wording, `noun` fills it.
// An action outside the list reads as its own words ("other" branch).
//   create send update status apply record schedule publish respond reply
//   upsert attach write remove

/** Resources with a better name than their segment (components.approvalGate.resource.*). */
const RESOURCE_LABELS: Record<string, string> = {
	billpayment: 'billpayment',
	creditmemo: 'creditmemo',
	journalentry: 'journalentry',
	purchaseorder: 'purchaseorder',
	po: 'purchaseorder',
	opportunity: 'opportunity',
	// The company's own files. Granting one of these is granting the employee the
	// right to change how the whole business works, so the row says which file.
	company: 'company',
	industry: 'industry',
	franchise: 'franchise',
};

/** "quality_checklist" / "purchase-order" → "quality checklist". */
function words(segment: string): string {
	return segment.split(/[-_]/).filter(Boolean).join(' ');
}

/** The same, opened with a capital: a heading or the start of a sentence. */
function humanize(segment: string): string {
	const w = words(segment);
	return w.charAt(0).toUpperCase() + w.slice(1);
}

/**
 * "ledger.billpayment.create" → "Create bill payment".
 *
 * Every label is DERIVED from the address, with the two registers above only
 * improving on the derivation — so an operation nobody here has ever seen (an
 * owner's own employee, an operation named in a pack's law) still reads as
 * words. A raw dotted address is never shown.
 *
 * A typed operation is written `capability.resource.action`, so the last
 * segment is the verb. A two-segment address is written verb-first
 * ("spend.above_company_bounds" — the shape a pack's law uses), so it reads in
 * the order it is written.
 */
export function operationLabel(operation: string): string {
	const parts = operation.split('.').filter(Boolean);
	if (parts.length === 0) return '';
	if (parts.length === 1) return humanize(parts[0]);
	const [resource, action] =
		parts.length === 2
			? [parts[1], parts[0]]
			: [parts[parts.length - 2], parts[parts.length - 1]];
	const tr = get(t);
	const known = RESOURCE_LABELS[resource];
	const noun = known ? tr(`components.approvalGate.resource.${known}`) : words(resource);
	return tr('components.approvalGate.operation', {
		values: { action, verb: humanize(action), noun }
	}).trim();
}
