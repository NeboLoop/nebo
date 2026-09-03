import { redirect } from '@sveltejs/kit';
// Under the management tunnel the app lives under /t/<botID>/; a bare
// redirect escapes onto the hub's own site.
import { withBase } from '$lib/nav';

export const load = () => {
	throw redirect(307, withBase('/marketplace?kind=plugins'));
};
