// Human labels for typed interface operations — the ONE source used by both the
// approval modal (ApprovalGate) and the per-employee Approvals settings page.
// Non-technical rule: an owner should never have to read "ledger.billpayment.create".

const OP_VERBS: Record<string, string> = {
	create: 'Create',
	send: 'Send',
	update: 'Update',
	status: 'Change status of',
	apply: 'Apply',
	record: 'Record',
	schedule: 'Schedule',
	publish: 'Publish',
	respond: 'Respond to',
	reply: 'Reply to',
	upsert: 'Save',
	attach: 'Attach',
	write: 'Write',
	remove: 'Remove',
};

const RESOURCE_LABELS: Record<string, string> = {
	billpayment: 'bill payment',
	creditmemo: 'credit memo',
	journalentry: 'journal entry',
	purchaseorder: 'purchase order',
	po: 'purchase order',
	opportunity: 'deal',
	// The company's own files. Granting one of these is granting the employee the
	// right to change how the whole business works, so the row says which file.
	company: 'the Company file — how this business runs',
	industry: 'the Industry file — how this trade works',
	franchise: 'the Franchise file — a brand\'s requirements',
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
	const verb = OP_VERBS[action] ?? humanize(action);
	const noun = RESOURCE_LABELS[resource] ?? words(resource);
	return `${verb} ${noun}`.trim();
}

/** Capability group → owner-readable heading. */
export const CAPABILITY_LABELS: Record<string, string> = {
	ledger: 'Accounting & money',
	mail: 'Email',
	sms: 'Text messages',
	esign: 'Contracts & signing',
	crm: 'Sales CRM',
	ats: 'Recruiting',
	store: 'Store & orders',
	social: 'Social media',
	cms: 'Website content',
	'email-marketing': 'Email campaigns',
	helpdesk: 'Support tickets',
	kb: 'Knowledge base',
	reviews: 'Reviews',
	ads: 'Advertising',
	tickets: 'Project issues',
	layers: "The company's own files",
};

export function capabilityLabel(capability: string): string {
	return CAPABILITY_LABELS[capability] ?? humanize(capability);
}
