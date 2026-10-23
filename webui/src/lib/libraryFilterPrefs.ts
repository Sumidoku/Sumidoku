import type { LibrarySort } from '../api/types';

// Library filters, search text, and sort order, kept in localStorage across sessions.
// Stale ids (since-deleted categories, uninstalled sources) are pruned by the page once the
// lists are loaded — a leftover include id would otherwise filter the library down to nothing
// with no chip showing why

export type TriState = 1 | -1;

export interface LibraryFilterPrefs {
	search: string;
	sort: LibrarySort;
	/** Boolean filter key (unread/downloaded/bookmarked) → state */
	filters: Record<string, TriState>;
	/** Content rating value (1/2/3) → state */
	ratings: Record<string, TriState>;
	/** Source id (string) → state */
	sources: Record<string, TriState>;
	/** Category id, "0" = uncategorized (string) → state */
	categories: Record<string, TriState>;
	/** The active filter group id; null = no group selected */
	group: number | null;
}

const KEY = 'sumidoku.library.filters';

const EMPTY: LibraryFilterPrefs = {
	search: '',
	sort: 'added',
	filters: {},
	ratings: {},
	sources: {},
	categories: {},
	group: null
};

/** False on the first visit ever: the server-side library.default_category only seeds that first visit — afterwards the stored selection always wins */
export function hasLibraryFilterPrefs(): boolean {
	try {
		return localStorage.getItem(KEY) !== null;
	} catch {
		return false;
	}
}

export function loadLibraryFilters(): LibraryFilterPrefs {
	try {
		const raw = localStorage.getItem(KEY);
		if (raw === null) return EMPTY;
		const v: unknown = JSON.parse(raw);
		if (!v || typeof v !== 'object') return EMPTY;
		const p = v as Partial<LibraryFilterPrefs>;
		return {
			search: typeof p.search === 'string' ? p.search : '',
			sort: cleanSort(p.sort),
			filters: clean(p.filters),
			ratings: clean(p.ratings),
			sources: clean(p.sources),
			categories: clean(p.categories),
			group: typeof p.group === 'number' ? p.group : null
		};
	} catch {
		// Private mode or broken JSON both count as absent, silently
		return EMPTY;
	}
}

function cleanSort(value: unknown): LibrarySort {
	if (value === 'title' || value === 'lastRead' || value === 'latestChapter' || value === 'unread') {
		return value;
	}
	return 'added';
}

/** Only 1/-1 are valid states; bad values (hand-edited storage) are dropped */
function clean(rec: unknown): Record<string, TriState> {
	const out: Record<string, TriState> = {};
	if (rec && typeof rec === 'object') {
		for (const [k, v] of Object.entries(rec)) {
			if (v === 1 || v === -1) out[k] = v;
		}
	}
	return out;
}

export function saveLibraryFilters(prefs: LibraryFilterPrefs): void {
	try {
		localStorage.setItem(KEY, JSON.stringify(prefs));
	} catch {
		// Private mode blocks the write; degrade silently (filters apply for this session only)
	}
}
