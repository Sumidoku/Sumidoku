// Tri-state filters (include/exclude) of the cross-source search page, kept in localStorage across sessions.
// The filters only decide which sources participate (client-side filtering; the server does not filter);
// the raw mapping is stored as-is — leftover ids of uninstalled sources simply never match, no cleanup needed

export type TriState = 1 | -1;

export interface GlobalSearchFilterPrefs {
	/** Content rating value (0/1/2) → state */
	ratings: Record<string, TriState>;
	/** Language code (lowercase) → state */
	langs: Record<string, TriState>;
	/** Source id (string) → state */
	sources: Record<string, TriState>;
}

const KEY = 'sumidoku.globalSearch.filters';

const EMPTY: GlobalSearchFilterPrefs = { ratings: {}, langs: {}, sources: {} };

export function loadGlobalSearchFilters(): GlobalSearchFilterPrefs {
	try {
		const raw = localStorage.getItem(KEY);
		if (raw === null) return EMPTY;
		const v: unknown = JSON.parse(raw);
		if (!v || typeof v !== 'object') return EMPTY;
		const p = v as Partial<GlobalSearchFilterPrefs>;
		return { ratings: clean(p.ratings), langs: clean(p.langs), sources: clean(p.sources) };
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

export function saveGlobalSearchFilters(prefs: GlobalSearchFilterPrefs): void {
	try {
		localStorage.setItem(KEY, JSON.stringify(prefs));
	} catch {
		// Private mode blocks the write; degrade silently (filters apply for this session only)
	}
}
