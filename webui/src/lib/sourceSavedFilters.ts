import type { FilterValue } from '../api/types';

// Named filter groups ("saved searches") of the source browse page, stored in localStorage per source.
// Aligns with Aidoku's SavedSearch: a name, the submitted query and the filter values.

export interface SavedFilterGroup {
	name: string;
	query: string | null;
	filters: FilterValue[];
}

export type SaveGroupResult =
	| { ok: true; groups: SavedFilterGroup[] }
	| { ok: false; reason: 'empty' | 'duplicate'; groups: SavedFilterGroup[] };

function storageKey(sourceId: number): string {
	return `sumidoku.sourceSavedFilters.${sourceId}`;
}

// Restored groups are re-validated: malformed entries are dropped whole and duplicate names keep the first, so a hand-edited store cannot break the panel
function sanitize(raw: unknown): SavedFilterGroup[] {
	if (!Array.isArray(raw)) return [];
	const out: SavedFilterGroup[] = [];
	for (const item of raw) {
		if (!item || typeof item !== 'object') continue;
		const rec = item as Record<string, unknown>;
		if (typeof rec.name !== 'string') continue;
		const name = rec.name.trim();
		if (!name || out.some((g) => g.name === name)) continue;
		const query =
			typeof rec.query === 'string' && rec.query.trim() !== '' ? rec.query.trim() : null;
		const filters = Array.isArray(rec.filters)
			? (rec.filters.filter((v) => v && typeof v === 'object') as FilterValue[])
			: [];
		out.push({ name, query, filters });
	}
	return out;
}

export function loadSavedFilters(sourceId: number): SavedFilterGroup[] {
	if (!Number.isFinite(sourceId)) return [];
	try {
		const raw = localStorage.getItem(storageKey(sourceId));
		if (raw === null) return [];
		return sanitize(JSON.parse(raw));
	} catch {
		// Private mode or broken JSON both count as absent, silently
		return [];
	}
}

export function saveSavedFilters(sourceId: number, groups: SavedFilterGroup[]): void {
	if (!Number.isFinite(sourceId)) return;
	try {
		// Aidoku removes the defaults key when the list empties
		if (groups.length === 0) localStorage.removeItem(storageKey(sourceId));
		else localStorage.setItem(storageKey(sourceId), JSON.stringify(groups));
	} catch {
		// Private mode blocks the write; groups last for this session only
	}
}

// New groups go at the end (Aidoku appends) and names are unique per source; a duplicate or empty name is reported back so the page can warn
export function addSavedFilter(
	sourceId: number,
	name: string,
	query: string | null,
	filters: FilterValue[]
): SaveGroupResult {
	const groups = loadSavedFilters(sourceId);
	const trimmed = name.trim();
	if (!trimmed) return { ok: false, reason: 'empty', groups };
	if (groups.some((g) => g.name === trimmed)) return { ok: false, reason: 'duplicate', groups };
	const next: SavedFilterGroup[] = [
		...groups,
		{ name: trimmed, query: query && query.trim() !== '' ? query.trim() : null, filters }
	];
	saveSavedFilters(sourceId, next);
	return { ok: true, groups: next };
}

export function removeSavedFilter(sourceId: number, name: string): SavedFilterGroup[] {
	const next = loadSavedFilters(sourceId).filter((g) => g.name !== name);
	saveSavedFilters(sourceId, next);
	return next;
}
