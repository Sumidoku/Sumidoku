import type { Chapter } from '../api/types';
import { toMillis } from './format';

// 章节列表的排序/筛选设置，按漫画存 localStorage，详情页与阅读器共用同一份

export type SortKey = 'source' | 'chapter' | 'date';
/** 0 = 关（不过滤）、1 = 含（只显示命中）、-1 = 排除（隐藏命中） */
export type Tri = 0 | 1 | -1;

export interface ChapterPrefs {
	sort: SortKey;
	/** false = 降序（详情页默认，最新章在上） */
	asc: boolean;
	filters: { unread: Tri; downloaded: Tri; bookmarked: Tri; locked: Tri };
	/** null = 全部语言 */
	language: string | null;
	/** 空数组 = 不按扫图组过滤；非空时任一命中即保留 */
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
		// 隐私模式读不到或坏 JSON 都回退默认，静默
		return { ...DEFAULT_PREFS };
	}
}

export function savePrefs(id: number, prefs: ChapterPrefs): void {
	if (!Number.isFinite(id)) return;
	try {
		localStorage.setItem(storageKey(id), JSON.stringify(prefs));
	} catch {
		// 隐私模式写不进存储，静默降级（设置仅本轮生效）
	}
}

/** 过滤条件：0 保留全部、1 保留真值、-1 保留假值 */
function tri(value: unknown, t: Tri): boolean {
	if (t === 0) return true;
	return t === 1 ? !!value : !value;
}

export function applyPrefs(chapters: Chapter[], prefs: ChapterPrefs): Chapter[] {
	const list = [...chapters];

	switch (prefs.sort) {
		case 'source':
			// 接口按源序升序返回，保持服务端 source_order 顺序
			if (!prefs.asc) list.reverse();
			break;
		case 'chapter':
			// 无章节号按 -1 处理（同 Aidoku），排在最前
			list.sort((a, b) => (a.chapter_number ?? -1) - (b.chapter_number ?? -1));
			if (!prefs.asc) list.reverse();
			break;
		case 'date': {
			// 无日期按 -Infinity 处理，稳定沉底
			const millis = (c: Chapter) => toMillis(c.date_uploaded) ?? -Infinity;
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
