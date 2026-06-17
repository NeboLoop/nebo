import { redirect } from '@sveltejs/kit';

// The one skills UI is /settings/skills (reads the real skill loader). This
// route used to render a parallel, broken list off the tool registry — removed.
export const load = () => {
	throw redirect(307, '/settings/skills');
};
