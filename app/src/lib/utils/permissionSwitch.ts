import type { PermissionSwitch } from '$lib/api/nebo';

/** The server's three switch values. */
export type SwitchValue = 'allow' | 'ask' | 'deny';

/** What a Permissions page's `set` takes: a value, or `inherit` to clear the page's own setting. */
export type SwitchChange = SwitchValue | 'inherit';

/** The three states, in order, with their short labels and the long forms for tooltips. */
export const switchStates: { value: SwitchValue; label: string; long: string }[] = [
	{ value: 'allow', label: 'permissions.allow', long: 'permissions.allowLong' },
	{ value: 'ask', label: 'permissions.ask', long: 'permissions.askLong' },
	{ value: 'deny', label: 'permissions.off', long: 'permissions.offLong' }
];

/**
 * What tapping `picked` on a switch sends, or null for nothing. Tapping the
 * lit state of the page's own setting clears it (back to the default or the
 * company's); tapping a lit inherited state makes it the page's own. A
 * locked switch sends nothing.
 */
export function switchChange(sw: PermissionSwitch, picked: SwitchValue): SwitchChange | null {
	if (sw.locked) return null;
	if (sw.value !== picked) return picked;
	if (!sw.inherited) return sw.canInherit ? 'inherit' : null;
	return picked;
}

/** The i18n key of the note beside an inherited switch, or null for the page's own setting. */
export function inheritedNote(sw: PermissionSwitch): string | null {
	if (sw.locked) return 'permissions.lockedByCompany';
	if (!sw.inherited) return null;
	return sw.inheritsFrom === 'company' ? 'permissions.sameAsCompany' : 'permissions.viaDefault';
}
