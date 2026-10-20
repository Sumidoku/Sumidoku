import type { FilterValue } from './api/types';

// 图源浏览页的筛选值，按源存 localStorage；下次进页回填，源改版后对不上的 id 在回填时被静默丢弃

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
		// 隐私模式读不到或坏 JSON 都视为没有，静默
		return [];
	}
}

export function saveFilterPrefs(sourceId: number, values: FilterValue[]): void {
	if (!Number.isFinite(sourceId)) return;
	try {
		localStorage.setItem(storageKey(sourceId), JSON.stringify(values));
	} catch {
		// 隐私模式写不进存储，静默降级（筛选仅本轮生效）
	}
}
