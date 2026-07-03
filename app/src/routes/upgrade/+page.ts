import { redirect } from '@sveltejs/kit';

// /upgrade moved to /pricing. Kept as a redirect so any stale link
// (notifications, marketplace prompts, muscle memory) still lands.
export function load() {
	redirect(301, '/pricing');
}
