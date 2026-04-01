import { browser } from '$app/environment';
import { init, register } from 'svelte-i18n';

// Register locale files (lazy-loaded)
register('en', () => import('./locales/en.json'));
register('de', () => import('./locales/de.json'));
register('es', () => import('./locales/es.json'));
register('pt-BR', () => import('./locales/pt-BR.json'));
register('zh-CN', () => import('./locales/zh-CN.json'));
register('zh-TW', () => import('./locales/zh-TW.json'));
register('ja', () => import('./locales/ja.json'));
register('ko', () => import('./locales/ko.json'));
register('fr', () => import('./locales/fr.json'));
register('hi', () => import('./locales/hi.json'));
register('it', () => import('./locales/it.json'));
register('pl', () => import('./locales/pl.json'));
register('tr', () => import('./locales/tr.json'));
register('vi', () => import('./locales/vi.json'));
register('ar', () => import('./locales/ar.json'));
register('uk', () => import('./locales/uk.json'));
register('ru', () => import('./locales/ru.json'));
register('nl', () => import('./locales/nl.json'));
register('id', () => import('./locales/id.json'));
register('th', () => import('./locales/th.json'));
register('ms', () => import('./locales/ms.json'));
register('he', () => import('./locales/he.json'));
register('sv', () => import('./locales/sv.json'));
register('pt', () => import('./locales/pt.json'));
register('bn', () => import('./locales/bn.json'));

const supportedLocales = ['en', 'de', 'es', 'pt-BR', 'zh-CN', 'zh-TW', 'ja', 'ko', 'fr', 'hi', 'it', 'pl', 'tr', 'vi', 'ar', 'uk', 'ru', 'nl', 'id', 'th', 'ms', 'he', 'sv', 'pt', 'bn'];

function detectLocale(): string {
	if (!browser) return 'en';
	const saved = localStorage.getItem('nebo_locale');
	if (saved) return saved;
	const browserLang = navigator.language;
	if (supportedLocales.includes(browserLang)) return browserLang;
	const base = browserLang.split('-')[0];
	return supportedLocales.find(l => l === base || l.startsWith(base + '-')) ?? 'en';
}

init({
	fallbackLocale: 'en',
	initialLocale: detectLocale()
});
