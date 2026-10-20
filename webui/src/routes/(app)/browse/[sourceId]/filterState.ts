import type { Filter, FilterValue } from '$lib/api/types';

export type CheckVal = 0 | 1 | -1;

export type SortVal = { index: number | null; ascending: boolean };

export interface FilterStates {
	textVals: Record<string, string>;
	sortVals: Record<string, SortVal>;
	checkVals: Record<string, CheckVal>;
	selectVals: Record<string, string>;
	multiVals: Record<string, Record<string, CheckVal>>;
	rangeVals: Record<string, { from: string; to: string }>;
}

export function initFilterStates(fs: Filter[]): FilterStates {
	const s: FilterStates = {
		textVals: {},
		sortVals: {},
		checkVals: {},
		selectVals: {},
		multiVals: {},
		rangeVals: {}
	};
	for (const f of fs) {
		if (f.type === 'text') {
			s.textVals[f.id] = '';
		} else if (f.type === 'sort') {
			const d = f.default as { index?: number; ascending?: boolean } | null | undefined;
			s.sortVals[f.id] = { index: d?.index ?? null, ascending: d?.ascending ?? true };
		} else if (f.type === 'check') {
			s.checkVals[f.id] = f.default ? 1 : 0;
		} else if (f.type === 'select') {
			s.selectVals[f.id] = typeof f.default === 'string' ? f.default : '';
		} else if (f.type === 'multi-select') {
			const rec: Record<string, CheckVal> = {};
			for (const v of f.default_included ?? []) rec[v] = 1;
			for (const v of f.default_excluded ?? []) rec[v] = -1;
			s.multiVals[f.id] = rec;
		} else if (f.type === 'range') {
			s.rangeVals[f.id] = { from: '', to: '' };
		}
	}
	return s;
}

// 选中值取 ids（有 ids 时）否则取选项名
export function optionKey(f: Filter, i: number): string {
	return f.ids?.[i] ?? f.options?.[i] ?? '';
}

// 源主页深链（?filters=...）带来的筛选值，解析失败视为没有
export function parseFiltersParam(raw: string | null): FilterValue[] {
	if (!raw) return [];
	try {
		const v: unknown = JSON.parse(raw);
		return Array.isArray(v) ? (v as FilterValue[]) : [];
	} catch {
		return [];
	}
}

// 深链筛选值回填面板草稿；filter id 对不上（源改版）的值静默丢弃
export function applyFilterValues(s: FilterStates, values: FilterValue[]): FilterStates {
	const next: FilterStates = {
		textVals: { ...s.textVals },
		sortVals: { ...s.sortVals },
		checkVals: { ...s.checkVals },
		selectVals: { ...s.selectVals },
		multiVals: { ...s.multiVals },
		rangeVals: { ...s.rangeVals }
	};
	for (const v of values) {
		if ('Text' in v) {
			next.textVals[v.Text.id] = v.Text.value;
		} else if ('Sort' in v) {
			next.sortVals[v.Sort.id] = { index: v.Sort.index, ascending: v.Sort.ascending };
		} else if ('Check' in v) {
			next.checkVals[v.Check.id] = v.Check.value as CheckVal;
		} else if ('Select' in v) {
			next.selectVals[v.Select.id] = v.Select.value;
		} else if ('MultiSelect' in v) {
			const rec: Record<string, CheckVal> = {};
			for (const k of v.MultiSelect.included) rec[k] = 1;
			for (const k of v.MultiSelect.excluded) rec[k] = -1;
			next.multiVals[v.MultiSelect.id] = rec;
		} else if ('Range' in v) {
			next.rangeVals[v.Range.id] = {
				from: v.Range.from === null ? '' : String(v.Range.from),
				to: v.Range.to === null ? '' : String(v.Range.to)
			};
		}
	}
	return next;
}

export function buildFilterValues(filters: Filter[] | null, draft: FilterStates | null): FilterValue[] {
	const out: FilterValue[] = [];
	if (!draft) return out;
	for (const f of filters ?? []) {
		if (f.type === 'text') {
			const v = (draft.textVals[f.id] ?? '').trim();
			if (v) out.push({ Text: { id: f.id, value: v } });
		} else if (f.type === 'sort') {
			const s = draft.sortVals[f.id];
			if (s && s.index !== null) {
				out.push({ Sort: { id: f.id, index: s.index, ascending: s.ascending } });
			}
		} else if (f.type === 'check') {
			const v = draft.checkVals[f.id] ?? 0;
			if (v !== 0) out.push({ Check: { id: f.id, value: v } });
		} else if (f.type === 'select') {
			const v = draft.selectVals[f.id] ?? '';
			if (v) out.push({ Select: { id: f.id, value: v } });
		} else if (f.type === 'multi-select') {
			const rec = draft.multiVals[f.id] ?? {};
			const included = Object.entries(rec)
				.filter(([, v]) => v === 1)
				.map(([k]) => k);
			const excluded = Object.entries(rec)
				.filter(([, v]) => v === -1)
				.map(([k]) => k);
			if (included.length || excluded.length) {
				out.push({ MultiSelect: { id: f.id, included, excluded } });
			}
		} else if (f.type === 'range') {
			const r = draft.rangeVals[f.id];
			if (!r) continue;
			const from = r.from.trim() === '' ? null : Number(r.from);
			const to = r.to.trim() === '' ? null : Number(r.to);
			if ((from !== null && !Number.isNaN(from)) || (to !== null && !Number.isNaN(to))) {
				out.push({ Range: { id: f.id, from, to } });
			}
		}
	}
	return out;
}

/** 三态循环的下一态（多选 chip 与面板共用） */
export function nextMultiVal(cur: CheckVal, canExclude: boolean): CheckVal {
	return cur === 0 ? 1 : cur === 1 ? (canExclude ? -1 : 0) : 0;
}

/** 草稿里该筛选是否会产生提交值（Aidoku enabledFilters 口径，用于启用排序） */
export function isFilterEnabled(f: Filter, states: FilterStates): boolean {
	switch (f.type) {
		case 'sort':
			return (states.sortVals[f.id]?.index ?? null) !== null;
		case 'check':
			return (states.checkVals[f.id] ?? 0) !== 0;
		case 'select':
			return (states.selectVals[f.id] ?? '') !== '';
		case 'multi-select':
			return Object.keys(states.multiVals[f.id] ?? {}).length > 0;
		default:
			return false;
	}
}
