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

init({
	fallbackLocale: 'en',
	initialLocale: browser ? (localStorage.getItem('nebo_locale') || 'en') : 'en'
});
