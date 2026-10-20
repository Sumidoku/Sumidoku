// 源元数据展示：语言码 → 人类可读名称，内容分级 → Aidoku 三档文案。
// 语言名跟随当前 UI 语言（svelte-i18n 的 locale），
// multi/all 是 Aidoku 的多语言哨兵，不在 Intl 里，单独给名。
import { get } from 'svelte/store';
import { _, locale } from 'svelte-i18n';

/** 非组件环境里的翻译取值（i18n 在根布局已 await 初始化） */
function t(key: string): string {
	try {
		return get(_)(key) as string;
	} catch {
		return key;
	}
}

function currentLocale(): string | undefined {
	try {
		return (get(locale) as string | null) ?? undefined;
	} catch {
		return undefined;
	}
}

function getDisplayNames(): Intl.DisplayNames | null {
	try {
		const loc = currentLocale();
		return new Intl.DisplayNames(loc ? [loc] : undefined, { type: 'language' });
	} catch {
		return null;
	}
}

/** 多语言源的哨兵码：Aidoku 用 multi，个别 index 用 all */
export const MULTI_LANGUAGE = 'multi';

const MULTI_CODES = new Set([MULTI_LANGUAGE, 'all']);

export function isMultiLanguage(code: string): boolean {
	return MULTI_CODES.has(code.trim().toLowerCase());
}

export function languageLabel(code: string): string {
	const normalized = code.trim().toLowerCase();
	if (MULTI_CODES.has(normalized)) return t('source.multiLanguage');
	return getDisplayNames()?.of(normalized) ?? code;
}

/** 按显示名排序 */
export function compareLanguageLabel(a: string, b: string): number {
	return languageLabel(a).localeCompare(languageLabel(b), 'und');
}

/** 已装源列表排序：按名字字母序，名字相同退回 key 保证稳定 */
export function compareSourceName(
	a: { name: string; key: string },
	b: { name: string; key: string }
): number {
	const byName = a.name.localeCompare(b.name, 'und');
	if (byName !== 0) return byName;
	if (a.key < b.key) return -1;
	if (a.key > b.key) return 1;
	return 0;
}

// Aidoku 的 SourceContentRating：0 安全 / 1 含 NSFW / 2 主要 NSFW
export interface ContentRatingLevel {
	value: number;
	/** 读取时翻译：getter 让 level.label 始终返回当前语言文案 */
	readonly label: string;
	color: 'accent' | 'warning' | 'danger';
}

export const CONTENT_RATING_LEVELS: ContentRatingLevel[] = [
	{ value: 0, get label() { return t('rating.sourceSafe'); }, color: 'accent' },
	{ value: 1, get label() { return t('rating.sourceNsfwSome'); }, color: 'warning' },
	{ value: 2, get label() { return t('rating.sourceNsfw'); }, color: 'danger' }
];

// 漫画级内容分级（manga.content_rating，Aidoku ContentRating）：1 安全 / 2 建议 / 3 NSFW；
// 与上方源级 0/1/2 是两套刻度，别混用
export const MANGA_CONTENT_RATING_LEVELS: ContentRatingLevel[] = [
	{ value: 1, get label() { return t('rating.mangaSafe'); }, color: 'accent' },
	{ value: 2, get label() { return t('rating.mangaSuggestive'); }, color: 'warning' },
	{ value: 3, get label() { return t('rating.mangaNsfw'); }, color: 'danger' }
];

function levelOf(value: number): ContentRatingLevel {
	if (value >= 0 && value < CONTENT_RATING_LEVELS.length) return CONTENT_RATING_LEVELS[value];
	// 越界（>2）按最高档显示；Aidoku 的 unknown(0) 也落在安全档
	return CONTENT_RATING_LEVELS[CONTENT_RATING_LEVELS.length - 1];
}

export function contentRatingLabel(value: number): string {
	return levelOf(value).label;
}

export function contentRatingColor(value: number): ContentRatingLevel['color'] {
	return levelOf(value).color;
}
