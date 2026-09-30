// An error on the chat banner can end on the page that explains it:
// "… Learn more: https://…". The server sends it as plain text, so a phone
// showing the text as is still has a URL to tap; the banner here turns the
// label into the link.

export interface ErrorLink {
	/** The error up to the link. */
	text: string;
	/** What the link says ("Learn more"). */
	label: string;
	url: string;
}

const TRAILING_LINK = /^([\s\S]*?)\s*([^.!?:]+):\s*(https:\/\/\S+)\s*$/;

/** The error split from the "Label: https://…" it ends on, or null when it
 * ends on no link. */
export function trailingLink(error: string): ErrorLink | null {
	const m = TRAILING_LINK.exec(error);
	if (!m) return null;
	return { text: m[1], label: m[2].trim(), url: m[3] };
}
