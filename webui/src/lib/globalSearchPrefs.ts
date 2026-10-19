// 跨源搜索页的三态筛选（包含/排除），存 localStorage 跨会话保留。
// 筛选只决定哪些源参与搜索（客户端过滤，服务端不过滤）；
// 存的是原始映射，已卸载源的残留 id 在匹配时自然落空，无需清理

export type TriState = 1 | -1;

export interface GlobalSearchFilterPrefs {
	/** 内容分级值（0/1/2）→ 态 */
	ratings: Record<string, TriState>;
	/** 语言码（小写）→ 态 */
	langs: Record<string, TriState>;
	/** 源 id（字符串）→ 态 */
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
		// 隐私模式读不到或坏 JSON 都视为没有，静默
		return EMPTY;
	}
}

/** 只收 1/-1 两个合法态，坏值（手改存储）丢弃 */
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
		// 隐私模式写不进存储，静默降级（筛选仅本轮生效）
	}
}
