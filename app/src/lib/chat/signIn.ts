/**
 * The sign-in card: an employee signing the owner in to a command-line tool
 * on the bot's computer (`send_input` with `sign_in`). It carries the link,
 * the code to type on the page when the tool shows one, and a field for the
 * code the tool asks back for when it asks. The owner's code goes to the
 * bot's terminal only; this card never keeps or shows it, only that it was
 * entered. Mirrors crates/tools/src/sign_in.rs.
 */

/** The widget type. Mirrors `sign_in::CARD`. */
export const SIGN_IN = 'sign_in';
/** The card's answer once the owner's code went in. Mirrors `sign_in::CODE_ENTERED`. */
export const CODE_ENTERED = 'code_entered';
/** The card's answer when the sign-in finished on its own. Mirrors `sign_in::SIGNED_IN`. */
export const SIGNED_IN = 'signed_in';

const SKIP = '__skip__';
const FAILED = 'failed:';

/** Where an answered sign-in card stands. */
export type SignInStatus =
	| { state: 'cancelled' }
	| { state: 'signedIn' }
	| { state: 'failed'; reason: string }
	| { state: 'entered' };

/** What an answered sign-in card says. Whatever the answer was, a code is
 *  never shown: anything but a cancel, a finish or a failure reads as
 *  "code entered". */
export function signInStatus(response: string): SignInStatus {
	if (response === SKIP) return { state: 'cancelled' };
	if (response === SIGNED_IN) return { state: 'signedIn' };
	if (response.startsWith(FAILED)) return { state: 'failed', reason: response.slice(FAILED.length) };
	return { state: 'entered' };
}

/** The pasted code as it is sent: its first line, trimmed. Empty when
 *  nothing is left. */
export function cleanCode(pasted: string): string {
	return (
		pasted
			.split(/\r?\n/)
			.map((l) => l.trim())
			.find((l) => l.length > 0) ?? ''
	);
}

/** Whether a card's widgets are a sign-in card. */
export function isSignInCard(widgets: { type?: string }[] | undefined): boolean {
	return widgets?.[0]?.type === SIGN_IN;
}

/** What the thread keeps as a card's answer once it was given here: a
 *  sign-in card keeps that the code was entered, never the code. */
export function answerKept(widgets: { type?: string }[] | undefined, value: string): string {
	if (!isSignInCard(widgets)) return value;
	return signInStatus(value).state === 'entered' ? CODE_ENTERED : value;
}
