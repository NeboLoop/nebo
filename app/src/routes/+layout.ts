import '$lib/i18n';
import { waitLocale } from 'svelte-i18n';

export const ssr = false;

// Block first render until the active locale's dictionary is loaded —
// $t() throws "Cannot format a message without first setting the initial
// locale" if any component formats before init/loading completes.
export async function load() {
	await waitLocale();
}
