// Billing and pricing live in one place: the NeboAI web app. The desktop
// app shows what you have left and sends you there to change a plan, top
// up, or update a card. Nothing here ever takes a payment.
export const WEB_BILLING_URL = 'https://neboai.com/app/billing';

export function openWebBilling(): void {
	window.open(WEB_BILLING_URL, '_blank', 'noopener,noreferrer');
}

/** Owner-facing plan label — "pro_plus" / "pro-plus" → "Pro Plus", not "Pro_plus". */
export function formatPlanName(plan: string | null | undefined): string {
	if (!plan?.trim()) return 'Free';
	const key = plan.trim().toLowerCase().replace(/-/g, '_');
	const known: Record<string, string> = {
		free: 'Free',
		pro: 'Pro',
		pro_plus: 'Pro Plus',
		team: 'Team',
		enterprise: 'Enterprise',
	};
	if (known[key]) return known[key];
	return key
		.split('_')
		.filter(Boolean)
		.map((w) => w.charAt(0).toUpperCase() + w.slice(1))
		.join(' ');
}
