// The Inbox is a shelf over the workspace, never a page of its own: a full
// page has no sidebar and no way back. Deep links (notifications, /inbox?m=id)
// stay real URLs and land on Team with the shelf open, selection intact.
// Redirect in load, not in a component effect, so the page never mounts.
import { redirect } from '@sveltejs/kit';
import { withBase } from '$lib/nav';

export function load({ url }: { url: URL }) {
	const m = url.searchParams.get('m');
	redirect(307, withBase(`/dashboard?inbox=1${m ? `&m=${encodeURIComponent(m)}` : ''}`));
}
