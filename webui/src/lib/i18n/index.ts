import { browser } from '$app/env';
import { init, locale, register, waitLocale } from 'svelte-i18n';

export const LOCALES = ['en', 'zh', 'fr', 'it'] as const;
export type Locale = (typeof LOCALES)[number];

export const LOCALE_LABELS: Record<Locale, string> = {
	en: 'English',
	zh: '中文',
	fr: 'Français',
	it: 'Italiano'
};

const STORAGE_KEY = 'sumidoku.locale';
const FALLBACK: Locale = 'en';

register('en', () => import('./locales/en.json'));
register('zh', () => import('./locales/zh.json'));
register('fr', () => import('./locales/fr.json'));
register('it', () => import('./locales/it.json'));

function normalize(tag: string | null | undefined): Locale | null {
	if (!tag) return null;
	const base = tag.toLowerCase().split('-')[0];
	return (LOCALES as readonly string[]).includes(base) ? (base as Locale) : null;
}

/** 首次访问：优先存储值，其次浏览器语言，最后回退 en */
function detectInitial(): Locale {
	if (!browser) return FALLBACK;
	try {
		const stored = normalize(localStorage.getItem(STORAGE_KEY));
		if (stored) return stored;
	} catch {
		// 隐私模式读不到，继续探测浏览器语言
	}
	return normalize(navigator.language) ?? FALLBACK;
}

export function setLocale(next: Locale) {
	locale.set(next);
	try {
		localStorage.setItem(STORAGE_KEY, next);
	} catch {
		// 隐私模式写不进去也照常切换，只是不持久化
	}
	if (browser) document.documentElement.lang = next;
}

/** 在根 +layout 里 await，确保首屏前初始语言字典已就绪，避免看到裸 key */
export async function setupI18n(): Promise<void> {
	const initial = detectInitial();
	init({ fallbackLocale: FALLBACK, initialLocale: initial });
	if (browser) document.documentElement.lang = initial;
	await waitLocale(initial);
}
