// 源元数据展示：语言码 → 人类可读名称，内容分级 → Aidoku 三档文案。
// 语言名沿用浏览器 Locale（与 Aidoku 的 SourceLanguage.displayName 同口径），
// multi/all 是 Aidoku 的多语言哨兵，不在 Intl 里，单独给名。

let displayNames: Intl.DisplayNames | null | undefined;

function getDisplayNames(): Intl.DisplayNames | null {
	if (displayNames === undefined) {
		try {
			displayNames = new Intl.DisplayNames(undefined, { type: 'language' });
		} catch {
			displayNames = null;
		}
	}
	return displayNames;
}

/** 多语言源的哨兵码：Aidoku 用 multi，个别 index 用 all */
export const MULTI_LANGUAGE = 'multi';

const MULTI_CODES = new Set([MULTI_LANGUAGE, 'all']);

export function isMultiLanguage(code: string): boolean {
	return MULTI_CODES.has(code.trim().toLowerCase());
}

export function languageLabel(code: string): string {
	const normalized = code.trim().toLowerCase();
	if (MULTI_CODES.has(normalized)) return '多语言';
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
	return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
}

// Aidoku 的 SourceContentRating：0 安全 / 1 含 NSFW / 2 主要 NSFW
export interface ContentRatingLevel {
	value: number;
	label: string;
	color: 'accent' | 'warning' | 'danger';
}

export const CONTENT_RATING_LEVELS: ContentRatingLevel[] = [
	{ value: 0, label: '安全', color: 'accent' },
	{ value: 1, label: '含 NSFW', color: 'warning' },
	{ value: 2, label: 'NSFW', color: 'danger' }
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
