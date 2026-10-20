// Source metadata display: language code → human-readable name, content rating → Aidoku's three-level labels.
// Language names come from Intl.DisplayNames in the app language (recreated when it changes);
// multi/all is Aidoku's multi-language sentinel, absent from Intl, so it gets a name of its own.
import i18n from '../i18n';

let displayNames: Intl.DisplayNames | null = null;
let displayNamesLang = '';

function getDisplayNames(): Intl.DisplayNames | null {
	const lang = i18n.language;
	if (!displayNames || displayNamesLang !== lang) {
		try {
			displayNames = new Intl.DisplayNames(lang, { type: 'language' });
		} catch {
			displayNames = null;
		}
		displayNamesLang = lang;
	}
	return displayNames;
}

/** Sentinel code for multi-language sources: Aidoku uses multi, some indexes use all */
export const MULTI_LANGUAGE = 'multi';

const MULTI_CODES = new Set([MULTI_LANGUAGE, 'all']);

export function isMultiLanguage(code: string): boolean {
	return MULTI_CODES.has(code.trim().toLowerCase());
}

export function languageLabel(code: string): string {
	const normalized = code.trim().toLowerCase();
	if (MULTI_CODES.has(normalized)) return i18n.t('source.multiLanguage');
	return getDisplayNames()?.of(normalized) ?? code;
}

/** Sort by display name */
export function compareLanguageLabel(a: string, b: string): number {
	return languageLabel(a).localeCompare(languageLabel(b), 'und');
}

/** Installed-source list ordering: alphabetical by name, falling back to key for stability on ties */
export function compareSourceName(
	a: { name: string; key: string },
	b: { name: string; key: string }
): number {
	const byName = a.name.localeCompare(b.name, 'und');
	if (byName !== 0) return byName;
	return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
}

// Aidoku's SourceContentRating: 0 safe / 1 contains NSFW / 2 primarily NSFW
export interface ContentRatingLevel {
	value: number;
	label: string;
	color: 'accent' | 'warning' | 'danger';
}

// label resolves through a getter so a language switch re-renders consumers that map the arrays directly, without changing ContentRatingLevel
export const CONTENT_RATING_LEVELS: ContentRatingLevel[] = [
	{ value: 0, get label() { return i18n.t('source.rating.safe'); }, color: 'accent' },
	{ value: 1, get label() { return i18n.t('source.rating.containsNsfw'); }, color: 'warning' },
	{ value: 2, get label() { return i18n.t('source.rating.nsfw'); }, color: 'danger' }
];

// Manga-level content rating (manga.content_rating, Aidoku ContentRating): 1 safe / 2 suggestive / 3 NSFW;
// a different scale from the source-level 0/1/2 above — don't mix them up
export const MANGA_CONTENT_RATING_LEVELS: ContentRatingLevel[] = [
	{ value: 1, get label() { return i18n.t('source.rating.safe'); }, color: 'accent' },
	{ value: 2, get label() { return i18n.t('source.rating.suggestive'); }, color: 'warning' },
	{ value: 3, get label() { return i18n.t('source.rating.nsfw'); }, color: 'danger' }
];

function levelOf(value: number): ContentRatingLevel {
	if (value >= 0 && value < CONTENT_RATING_LEVELS.length) return CONTENT_RATING_LEVELS[value];
	// Out-of-range (>2) shows as the highest level; Aidoku's unknown(0) also lands on the safe level
	return CONTENT_RATING_LEVELS[CONTENT_RATING_LEVELS.length - 1];
}

export function contentRatingLabel(value: number): string {
	return levelOf(value).label;
}

export function contentRatingColor(value: number): ContentRatingLevel['color'] {
	return levelOf(value).color;
}
