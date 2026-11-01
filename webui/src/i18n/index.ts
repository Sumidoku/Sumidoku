import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

export const LANGUAGE_OPTIONS = [
	{ value: 'en', label: 'English' },
	{ value: 'de', label: 'Deutsch' },
	{ value: 'es', label: 'Español' },
	{ value: 'fr', label: 'Français' },
	{ value: 'it', label: 'Italiano' },
	{ value: 'ja', label: '日本語' },
	{ value: 'ko', label: '한국어' },
	{ value: 'ru', label: 'Русский' },
	{ value: 'zh-CN', label: '中文（简体）' },
	{ value: 'zh-TW', label: '中文（繁體）' }
] as const;

export type Language = (typeof LANGUAGE_OPTIONS)[number]['value'];

const LANGUAGES: readonly Language[] = LANGUAGE_OPTIONS.map((option) => option.value);

const STORAGE_KEY = 'sumidoku.language';

function detectLanguage(): Language {
	try {
		const stored = localStorage.getItem(STORAGE_KEY);
		if (LANGUAGES.includes(stored as Language)) return stored as Language;
	} catch {
		// Private mode blocks the read; fall through to the browser language
	}
	const nav = navigator.language.toLowerCase().replace(/_/g, '-');
	if (nav.startsWith('zh')) return /hant|-tw|-hk|-mo/.test(nav) ? 'zh-TW' : 'zh-CN';
	const primary = nav.split('-')[0];
	return LANGUAGES.find((lang) => lang.split('-')[0] === primary) ?? 'en';
}

// Catalogs live one file per namespace per locale (locales/<lang>/<ns>.json, the file
// name matching its single top-level key) so parallel edits never share a file; the
// glob auto-registers new namespaces without touching this file
const modules = import.meta.glob<{ default: Record<string, unknown> }>('./locales/*/*.json', {
	eager: true
});
const resources: Record<string, { translation: Record<string, unknown> }> = {};
for (const [path, mod] of Object.entries(modules)) {
	const lang = path.split('/')[2];
	resources[lang] ??= { translation: {} };
	Object.assign(resources[lang].translation, mod.default);
}

void i18n.use(initReactI18next).init({
	resources,
	lng: detectLanguage(),
	fallbackLng: 'en',
	interpolation: {
		// React already escapes interpolated values
		escapeValue: false
	}
});

export function setLanguage(lang: Language) {
	try {
		localStorage.setItem(STORAGE_KEY, lang);
	} catch {
		// Private mode blocks the write but the language still switches, just without persisting
	}
	void i18n.changeLanguage(lang);
}

/** Toast-friendly error text by stable code; a code outside the catalog keeps the raw server message */
export function translateApiError(
	code: string,
	serverMessage: string,
	vars?: Record<string, unknown>
): string {
	const key = `errors.${code}`;
	return i18n.exists(key) ? i18n.t(key, vars) : serverMessage;
}

export default i18n;
