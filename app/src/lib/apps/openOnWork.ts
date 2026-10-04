/**
 * `window.open_on_work`: an app opens over its chat the moment its employee
 * writes that chat's record. The employee's `app_data` writes broadcast
 * `app_data_changed`; a `chat:` key is stored as `chat:<chatId>:<name>`, so
 * the changed key names the chat being worked on.
 */

export interface AppDataChanged {
	appId?: string;
	keys?: string[];
	source?: string;
}

/** Whether this event is the employee of app `appId` writing chat `chat`'s record. */
export function worksOnChat(event: AppDataChanged | null | undefined, appId: string, chat: string): boolean {
	if (!event || !appId || !chat) return false;
	if (event.appId !== appId || event.source !== 'employee') return false;
	const prefix = `chat:${chat}:`;
	return (event.keys ?? []).some((k) => k.startsWith(prefix));
}
