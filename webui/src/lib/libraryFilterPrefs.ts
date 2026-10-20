// Tri-state chip states of the library page's filter row, kept in localStorage across sessions.
// The search input and the sort select stay session-local, same as the reader settings.
// Stale ids (since-deleted categories, uninstalled sources) are pruned by the page once the
// lists are loaded — a leftover include id would otherwise filter the library down to nothing
// with no chip showing why

export type TriState = 1 | -1;

export interface LibraryFilterPrefs {
	/** Boolean filter key (unread/downloaded/bookmarked) → state */
	filters: Record<string, TriState>;
	/** Content rating value (1/2/3) → state */
	ratings: Record<string, TriState>;
	/** Source id (string) → state */
	sources: Record<string, TriState>;
	/** Category id, "0" = uncategorized (string) → state */
	categories: Record<string, TriState>;
}

const KEY = 'sumidoku.library.filters';

const EMPTY: LibraryFilterPrefs = { filters: {}, ratings: {}, sources: {}, categories: {} };

export function loadLibraryFilters(): LibraryFilterPrefs {
	try {
		const raw = localStorage.getItem(KEY);
		if (raw === null) return EMPTY;
		const v: unknown = JSON.parse(raw);
		if (!v || typeof v !== 'object') return EMPTY;
		const p = v as Partial<LibraryFilterPrefs>;
		return { filters: clean(p.filters), ratings: clean(p.ratings), sources: clean(p.sources), categories: clean(p.categories) };
	} catch {
		// Private mode or broken JSON both count as absent, silently
		return EMPTY;
	}
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
