import { redirect } from '@sveltejs/kit';
// Under the management tunnel the app lives under /t/<botID>/; a bare
// redirect escapes onto the hub's own site.
import { withBase } from '$lib/nav';

// The one skills UI is /settings/skills (reads the real skill loader). This
// route used to render a parallel, broken list off the tool registry — removed.
export const load = () => {
	throw redirect(307, withBase('/settings/skills'));
};
