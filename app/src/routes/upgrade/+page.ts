import { redirect } from '@sveltejs/kit';
// Under the management tunnel the app lives under /t/<botID>/; a bare
// redirect escapes onto the hub's own site.
import { withBase } from '$lib/nav';

// Plans and payments live on neboai.com. Any stale /upgrade link lands on
// the in-app billing page, which sends people there.
export function load() {
	redirect(301, withBase('/settings/billing'));
}
