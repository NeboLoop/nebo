/** The languages Nebo speaks, each in its own name. The ONE list: the
 *  locale registry, the pickers and the locale tests all read it. */
export const LANGUAGES = [
	{ code: 'en', label: 'English' },
	{ code: 'de', label: 'Deutsch' },
	{ code: 'es', label: 'Español' },
	{ code: 'fr', label: 'Français' },
	{ code: 'it', label: 'Italiano' },
	{ code: 'pt', label: 'Português' },
	{ code: 'pt-BR', label: 'Português (Brasil)' },
	{ code: 'nl', label: 'Nederlands' },
	{ code: 'sv', label: 'Svenska' },
	{ code: 'pl', label: 'Polski' },
	{ code: 'tr', label: 'Türkçe' },
	{ code: 'ru', label: 'Русский' },
	{ code: 'uk', label: 'Українська' },
	{ code: 'ar', label: 'العربية' },
	{ code: 'he', label: 'עברית' },
	{ code: 'hi', label: 'हिन्दी' },
	{ code: 'bn', label: 'বাংলা' },
	{ code: 'th', label: 'ไทย' },
	{ code: 'vi', label: 'Tiếng Việt' },
	{ code: 'id', label: 'Bahasa Indonesia' },
	{ code: 'ms', label: 'Bahasa Melayu' },
	{ code: 'ja', label: '日本語' },
	{ code: 'ko', label: '한국어' },
	{ code: 'zh-CN', label: '简体中文' },
	{ code: 'zh-TW', label: '繁體中文' }
] as const;

export type LanguageCode = (typeof LANGUAGES)[number]['code'];

const RTL = new Set<string>(['ar', 'he']);

/** Whether a language reads right to left. */
export function isRtl(code: string | null | undefined): boolean {
	return !!code && RTL.has(code.split('-')[0]);
}
