/** What `employeeLanding` reads: the employee and its conversations. */
export interface LandingApi {
	getAgent(id: string): Promise<{ memoryMode?: unknown } | null | undefined>;
	listAgentChats(id: string): Promise<{ chats?: { id: string }[] } | null | undefined>;
}

/**
 * Where a click on an employee lands: an isolated employee's list of
 * matters, everyone else's latest conversation, and the new-chat page only
 * when the employee truly has none. An app is an employee like any other:
 * it opens its conversation (Open App lives in the chat header). It used to
 * open /overview, which only redirected on to the NEW-chat page, so the
 * first click on an app showed "New chat" and only a second click — once
 * the sidebar had learned the chat list — reached the conversation.
 *
 * Both requests are awaited, however slow: the answer is never guessed
 * from a list that has not arrived.
 */
export async function employeeLanding(api: LandingApi, id: string): Promise<string> {
	const [detail, chats] = await Promise.all([
		api.getAgent(id).catch(() => null),
		api.listAgentChats(id).catch(() => null)
	]);
	const mode = detail?.memoryMode;
	if (mode === 'separate' || mode === 'confidential') {
		return `/${id}/threads?list=${encodeURIComponent(id)}`;
	}
	const latest = chats?.chats?.[0]?.id;
	return latest ? `/${id}/threads/${latest}` : `/${id}/threads`;
}
