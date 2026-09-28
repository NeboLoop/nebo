/**
 * Who started a piece of work, and so which client its surface belongs to.
 *
 * Every WS event reaches every connected client: this desktop, a second
 * window, the phone. An interactive surface (an install's progress and
 * setup, a sign-in window, an approval) belongs to the client that started
 * the work. The owner hired from his phone and came back to a desktop full
 * of install dialogs stuck midway (2026-09-27).
 *
 * This page names itself once, `clientId`: on every API request (the
 * `X-Nebo-Client` header, `api/gocliRequest.ts`) and in the socket's
 * handshake (`websocket/client.ts`). The server stamps events about work
 * with the `client_id` that asked (null when no client on this bot asked:
 * a hire on the owner's account, a channel, an employee's own call, a
 * schedule) and the `session_id` it was asked in (`EventOrigin` in
 * crates/server/src/handlers/ws.rs). `opensHere` is the ONE place a
 * component asks whether to open a surface for an event.
 */

/** The request header this page names itself with. */
export const CLIENT_HEADER = 'X-Nebo-Client';

/** This page's name for the life of the page, the same across reconnects. */
export const clientId: string = crypto.randomUUID();

/** The origin fields the server stamps on an event about work. */
export interface EventOrigin {
	client_id?: string | null;
	session_id?: string | null;
}

/**
 * What to do with work no client on this bot asked for.
 *  - 'nowhere': a surface for it needs the person who started it: an install's
 *    setup, a sign-in. It opens on no client; the result still shows everywhere.
 *  - 'everywhere': it waits on the owner and nobody started it from a screen
 *    (a scheduled run's approval): it opens wherever the owner is, and the
 *    first answer anywhere closes it everywhere.
 */
export type Unclaimed = 'nowhere' | 'everywhere';

/** May this client open an interactive surface for this event? Only the client
 *  that started the work does; `unclaimed` says what happens when none did. */
export function opensHere(event: EventOrigin | null | undefined, unclaimed: Unclaimed): boolean {
	const owner = event?.client_id;
	if (!owner) return unclaimed === 'everywhere';
	return owner === clientId;
}
