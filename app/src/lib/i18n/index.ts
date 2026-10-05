import { browser } from '$app/environment';
import { storage } from '$lib/storage';
import { init, locale, register } from 'svelte-i18n';
import { isRtl, LANGUAGES } from './languages';

export { LANGUAGES, isRtl } from './languages';

// Locale files, lazy-loaded: one chunk per language.
const loaders = import.meta.glob<Record<string, unknown>>('./locales/*.json', { import: 'default' });
for (const { code } of LANGUAGES) {
	register(code, () => loaders[`./locales/${code}.json`]());
}

const supportedLocales: string[] = LANGUAGES.map((l) => l.code);

/** The ONE key the chosen language is kept under (base-scoped storage). */
const LOCALE_KEY = 'nebo_locale';

function detectLocale(): string {
	if (!browser) return 'en';
	const saved = storage.get(LOCALE_KEY);
	if (saved && supportedLocales.includes(saved)) return saved;
	const browserLang = navigator.language;
	if (supportedLocales.includes(browserLang)) return browserLang;
	const base = browserLang.split('-')[0];
	return supportedLocales.find((l) => l === base || l.startsWith(base + '-')) ?? 'en';
}

init({
	fallbackLocale: 'en',
	initialLocale: detectLocale()
});

// The document follows the language: `lang` for fonts, hyphenation and screen
// readers, `dir` so Arabic and Hebrew lay out right to left.
if (browser) {
	locale.subscribe((code) => {
		if (!code) return;
		const root = document.documentElement;
		root.lang = code;
		root.dir = isRtl(code) ? 'rtl' : 'ltr';
	});
}

/** Switch the app's language and keep the choice: this install's storage,
 *  and the owner's preference on the server. */
export async function setLanguage(code: string): Promise<void> {
	if (!supportedLocales.includes(code)) return;
	storage.set(LOCALE_KEY, code);
	await locale.set(code);
	try {
		const api = await import('$lib/api/nebo');
		await api.userUpdatePreferences({ language: code });
	} catch {
		// The choice is already kept on this install; the server copy is best-effort.
	}
}
