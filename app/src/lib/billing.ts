// Billing and pricing live in one place: the NeboAI web app. The desktop
// app shows what you have left and sends you there to change a plan, top
// up, or update a card. Nothing here ever takes a payment.
export const WEB_BILLING_URL = 'https://neboai.com/app/billing';

export function openWebBilling(): void {
	window.open(WEB_BILLING_URL, '_blank', 'noopener,noreferrer');
}
