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
};

const RESOURCE_LABELS: Record<string, string> = {
	billpayment: 'bill payment',
	creditmemo: 'credit memo',
	journalentry: 'journal entry',
	purchaseorder: 'purchase order',
	po: 'purchase order',
	opportunity: 'deal',
};

/** "ledger.billpayment.create" → "Create bill payment" */
export function operationLabel(operation: string): string {
	const parts = operation.split('.');
	const [resource, action] = [parts[parts.length - 2] ?? '', parts[parts.length - 1] ?? ''];
	const verb = OP_VERBS[action] ?? action.charAt(0).toUpperCase() + action.slice(1);
	const noun = RESOURCE_LABELS[resource] ?? resource;
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
};

export function capabilityLabel(capability: string): string {
	return CAPABILITY_LABELS[capability] ?? capability;
}
