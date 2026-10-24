// What kind of question an ask is, for how the chat shows it. The server
// reads the owner's next message in the conversation against the question
// waiting there (`handlers::asks`): an open question takes it as the answer,
// whatever it says; a card with options reads it as a pick or his own words;
// a card done in the app (an install, a sign-in, a hire) is never answered
// in words.

import type { AskWidgetDef } from '$lib/components/chat/AskWidget.svelte';

const IN_APP = new Set<AskWidgetDef['type']>(['connect_account', 'install_plugin', 'hire_employee']);

/** A question with nothing to pick: it shows as the employee's message, and
 *  the owner answers it in the composer, like any reply. */
export function isOpenQuestion(widgets: AskWidgetDef[] | undefined): boolean {
	const w = widgets?.[0];
	return !!w && widgets?.length === 1 && !IN_APP.has(w.type) && !(w.options?.length);
}

/** A question card: something to pick, or several questions to step
 *  through. While it waits it takes the composer's place; a card done in
 *  the app (an install, a sign-in, a hire) stays in the chat. */
export function takesOverComposer(widgets: AskWidgetDef[] | undefined): boolean {
	return !!widgets?.length && takesWords(widgets) && !isOpenQuestion(widgets);
}

/** Whether the owner's typed message can answer it: every question but a
 *  card done in the app. */
export function takesWords(widgets: AskWidgetDef[] | undefined): boolean {
	const w = widgets?.[0];
	return !w || !IN_APP.has(w.type);
}
