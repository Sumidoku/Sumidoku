import type { ChapterWithDownload } from '../api/types';
import { toMillis } from './format';

// Chapter list sort/filter prefs, stored in localStorage per manga; the detail page and the reader share the same entry

export type SortKey = 'source' | 'chapter' | 'date';
/** 0 = off (no filtering), 1 = include (show only matches), -1 = exclude (hide matches) */
export type Tri = 0 | 1 | -1;

export interface ChapterPrefs {
	sort: SortKey;
	/** false = descending (default: newest chapters on top) */
	asc: boolean;
	filters: { unread: Tri; downloaded: Tri; bookmarked: Tri; locked: Tri };
	/** null = all languages */
	language: string | null;
	/** Empty array = don't filter by scanlation group; otherwise any match keeps the chapter */
	scanlators: string[];
}

export const DEFAULT_PREFS: ChapterPrefs = {
	sort: 'source',
	asc: false,
	filters: { unread: 0, downloaded: 0, bookmarked: 0, locked: 0 },
	language: null,
	scanlators: []
};

export function nextTri(t: Tri): Tri {
	return t === 0 ? 1 : t === 1 ? -1 : 0;
}

function storageKey(id: number): string {
	return `sumidoku.chapterPrefs.${id}`;
}

function isTri(v: unknown): v is Tri {
	return v === 0 || v === 1 || v === -1;
}

function normalize(raw: unknown): ChapterPrefs {
	if (typeof raw !== 'object' || raw === null) return { ...DEFAULT_PREFS };
	const o = raw as Record<string, unknown>;
	const f =
		typeof o.filters === 'object' && o.filters !== null
			? (o.filters as Record<string, unknown>)
			: {};
	return {
		sort: o.sort === 'source' || o.sort === 'chapter' || o.sort === 'date' ? o.sort : DEFAULT_PREFS.sort,
		asc: typeof o.asc === 'boolean' ? o.asc : DEFAULT_PREFS.asc,
		filters: {
			unread: isTri(f.unread) ? f.unread : 0,
			downloaded: isTri(f.downloaded) ? f.downloaded : 0,
			bookmarked: isTri(f.bookmarked) ? f.bookmarked : 0,
			locked: isTri(f.locked) ? f.locked : 0
		},
		language: typeof o.language === 'string' ? o.language : null,
		scanlators: Array.isArray(o.scanlators)
			? o.scanlators.filter((s): s is string => typeof s === 'string')
			: []
	};
}

export function loadPrefs(id: number): ChapterPrefs {
	if (!Number.isFinite(id)) return { ...DEFAULT_PREFS };
	try {
		const raw = localStorage.getItem(storageKey(id));
		if (raw === null) return { ...DEFAULT_PREFS };
		return normalize(JSON.parse(raw));
	} catch {
		// Private mode or broken JSON both fall back to defaults, silently
		return { ...DEFAULT_PREFS };
	}
}

export function savePrefs(id: number, prefs: ChapterPrefs): void {
	if (!Number.isFinite(id)) return;
	try {
		localStorage.setItem(storageKey(id), JSON.stringify(prefs));
	} catch {
		// Private mode blocks the write; degrade silently (prefs apply for this session only)
	}
}

/** Filter condition: 0 keeps everything, 1 keeps truthy, -1 keeps falsy */
function tri(value: unknown, t: Tri): boolean {
	if (t === 0) return true;
	return t === 1 ? !!value : !value;
}

export function applyPrefs(chapters: ChapterWithDownload[], prefs: ChapterPrefs): ChapterWithDownload[] {
	const list = [...chapters];

	switch (prefs.sort) {
		case 'source':
			// The API returns source_order ASC, which sources list newest-first;
			// descending (the default) shows that order as-is, ascending flips it to oldest-first
			if (prefs.asc) list.reverse();
			break;
		case 'chapter':
			// Chapters without a number count as -1 (same as Aidoku) and sort first
			list.sort((a, b) => (a.chapter_number ?? -1) - (b.chapter_number ?? -1));
			if (!prefs.asc) list.reverse();
			break;
		case 'date': {
			// Missing dates count as -Infinity, so they sink to the bottom
			const millis = (c: ChapterWithDownload) => toMillis(c.date_uploaded) ?? -Infinity;
			list.sort((a, b) => millis(a) - millis(b));
			if (!prefs.asc) list.reverse();
			break;
		}
	}

	const { unread, downloaded, bookmarked, locked } = prefs.filters;
	return list.filter((c) => {
		if (!tri(!c.isRead, unread)) return false;
		if (!tri(c.isDownloaded, downloaded)) return false;
		if (!tri(c.isBookmarked, bookmarked)) return false;
		if (!tri(c.locked, locked)) return false;
		if (prefs.language !== null && c.language !== prefs.language) return false;
		if (prefs.scanlators.length > 0) {
			const groups = c.scanlators ?? [];
			if (!prefs.scanlators.some((s) => groups.includes(s))) return false;
		}
		return true;
	});
}
