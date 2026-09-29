import type { EnrichedChat } from '$lib/types/agentPage';

type Translate = (key: string, opts: { values: Record<string, string> }) => string;

/** One employee's conversations, as its list shows them. `own` is the
 *  owner's (the list, and the one the employee opens on); `teammates` is
 *  its threads with colleagues and teams, for the collapsed "With teammates"
 *  section. The server decides which is which. */
export function conversationLists(resp: { chats?: EnrichedChat[]; teammates?: EnrichedChat[] } | null | undefined): {
	own: EnrichedChat[];
	teammates: EnrichedChat[];
} {
	return { own: resp?.chats ?? [], teammates: resp?.teammates ?? [] };
}

/** Who is on the other side of a teammate thread: the colleague's name
 *  ("Top Coder"), or the team's as a team ("Development team"). */
export function teammateName(chat: Pick<EnrichedChat, 'kind' | 'with' | 'title'>, t: Translate): string {
	const name = chat.with || chat.title;
	if (chat.kind !== 'team' || /\bteam$/i.test(name)) return name;
	return t('sidebar.teamThread', { values: { name } });
}

/** A teammate thread's row: "With Top Coder", or "Development team". */
export function teammateLabel(chat: Pick<EnrichedChat, 'kind' | 'with' | 'title'>, t: Translate): string {
	const name = teammateName(chat, t);
	return chat.kind === 'team' ? name : t('sidebar.withTeammate', { values: { name } });
}
