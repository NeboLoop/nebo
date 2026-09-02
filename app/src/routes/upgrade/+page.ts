import { redirect } from '@sveltejs/kit';

// Plans and payments live on neboai.com. Any stale /upgrade link lands on
// the in-app billing page, which sends people there.
export function load() {
	redirect(301, '/settings/billing');
}
