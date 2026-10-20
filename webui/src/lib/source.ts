// Source metadata display: language code → human-readable name, content rating → Aidoku's three-level labels.
// Language names come from the browser Locale (same convention as Aidoku's SourceLanguage.displayName);
// multi/all is Aidoku's multi-language sentinel, absent from Intl, so it gets a name of its own.

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

/** Sentinel code for multi-language sources: Aidoku uses multi, some indexes use all */
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

export const CONTENT_RATING_LEVELS: ContentRatingLevel[] = [
	{ value: 0, label: '安全', color: 'accent' },
	{ value: 1, label: '含 NSFW', color: 'warning' },
	{ value: 2, label: 'NSFW', color: 'danger' }
];

// Manga-level content rating (manga.content_rating, Aidoku ContentRating): 1 safe / 2 suggestive / 3 NSFW;
// a different scale from the source-level 0/1/2 above — don't mix them up
export const MANGA_CONTENT_RATING_LEVELS: ContentRatingLevel[] = [
	{ value: 1, label: '安全', color: 'accent' },
	{ value: 2, label: '建议', color: 'warning' },
	{ value: 3, label: 'NSFW', color: 'danger' }
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
