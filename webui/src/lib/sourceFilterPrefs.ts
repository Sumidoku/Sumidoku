import type { FilterValue } from '../api/types';

// Filter values of the source browse page, stored in localStorage per source; restored on next visit, and ids that no longer match after a source update are silently dropped on restore

function storageKey(sourceId: number): string {
	return `sumidoku.sourceFilters.${sourceId}`;
}

export function loadFilterPrefs(sourceId: number): FilterValue[] {
	if (!Number.isFinite(sourceId)) return [];
	try {
		const raw = localStorage.getItem(storageKey(sourceId));
		if (raw === null) return [];
		const v: unknown = JSON.parse(raw);
		return Array.isArray(v) ? (v as FilterValue[]) : [];
	} catch {
		// Private mode or broken JSON both count as absent, silently
		return [];
	}
}

export function saveFilterPrefs(sourceId: number, values: FilterValue[]): void {
	if (!Number.isFinite(sourceId)) return;
	try {
		localStorage.setItem(storageKey(sourceId), JSON.stringify(values));
	} catch {
		// Private mode blocks the write; degrade silently (filters apply for this session only)
	}
}
