import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Search, SearchX, TriangleAlert } from 'lucide-react';
import { sourceImageUrl } from '../api/client';
import { useGlobalSearch, useRegisterManga, useSources } from '../api/hooks';
import type { Manga, Source, SourceSearchResult } from '../api/types';
import { statusText } from '../lib/format';
import {
	type GlobalSearchFilterPrefs,
	type TriState,
	loadGlobalSearchFilters,
	saveGlobalSearchFilters
} from '../lib/globalSearchPrefs';
import {
	CONTENT_RATING_LEVELS,
	MULTI_LANGUAGE,
	compareLanguageLabel,
	compareSourceName,
	isMultiLanguage,
	languageLabel
} from '../lib/source';
import { Button, CoverCard, EmptyState, Input, Skeleton, TriStateDropdown } from '../components';

type FilterGroup = 'ratings' | 'langs' | 'sources';

/** 三态循环的下一态：未选 → 包含 → 排除 → 未选 */
function nextTri(cur: TriState | 0): TriState | 0 {
	return cur === 0 ? 1 : cur === 1 ? -1 : 0;
}

/** 三态单维度判定：有包含项时条目须命中其一，命中任一排除项即出局 */
function triAllows(states: Record<string, TriState>, keys: string[]): boolean {
	let includes = 0;
	let includeHit = false;
	for (const [k, v] of Object.entries(states)) {
		if (v === 1) {
			includes += 1;
			if (keys.includes(k)) includeHit = true;
		} else if (keys.includes(k)) {
			return false;
		}
	}
	return includes === 0 || includeHit;
}

function sourceMatches(s: Source, f: GlobalSearchFilterPrefs): boolean {
	return (
		triAllows(f.ratings, [String(s.contentRating)]) &&
		triAllows(
			f.langs,
			s.languages.map((l) => l.trim().toLowerCase())
		) &&
		triAllows(f.sources, [String(s.id)])
	);
}

export default function SearchPage() {
	const navigate = useNavigate();
	const sourcesQuery = useSources();
	const [filters, setFilters] = useState<GlobalSearchFilterPrefs>(loadGlobalSearchFilters);
	useEffect(() => saveGlobalSearchFilters(filters), [filters]);

	// 输入防抖 500ms，与源内搜索一致
	const [input, setInput] = useState('');
	const [query, setQuery] = useState('');
	useEffect(() => {
		const t = setTimeout(() => setQuery(input.trim()), 500);
		return () => clearTimeout(t);
	}, [input]);

	// 停用源不参与也不进筛选候选；展示与候选顺序按源名（浏览页同口径）
	const sources = useMemo(
		() => (sourcesQuery.data ?? []).filter((s) => s.enabled).sort(compareSourceName),
		[sourcesQuery.data]
	);
	const langOptions = useMemo(() => {
		const set = new Set<string>();
		for (const s of sources) {
			for (const l of s.languages) {
				const code = l.trim().toLowerCase();
				if (code && !isMultiLanguage(code)) set.add(code);
			}
		}
		// 与 Aidoku 一致：多语言哨兵恒在列表首位
		return [MULTI_LANGUAGE, ...[...set].sort(compareLanguageLabel)];
	}, [sources]);

	const selected = useMemo(() => sources.filter((s) => sourceMatches(s, filters)), [sources, filters]);
	const sourceIds = useMemo(() => selected.map((s) => s.id), [selected]);

	const search = useGlobalSearch(sourceIds, query, query !== '' && sourceIds.length > 0);
	const resultsById = useMemo(() => {
		const map = new Map<number, SourceSearchResult>();
		for (const r of search.data?.sources ?? []) map.set(r.sourceId, r);
		return map;
	}, [search.data]);

	const register = useRegisterManga();
	// busy 态用 `<sourceId>:<key>` 复合键：不同源可能撞同 key，单 key 会互相误标
	const [pendingKey, setPendingKey] = useState<string | null>(null);

	async function openEntry(m: Manga) {
		if (m.id) {
			navigate(`/manga/${m.id}`);
			return;
		}
		// 聚合结果的条目都带着自己的 sourceId，登记必须落到它的属主源上
		const owner = m.sourceId;
		if (owner === undefined || pendingKey) return;
		setPendingKey(`${owner}:${m.key}`);
		try {
			const { id } = await register.mutateAsync({ sourceId: owner, manga: m });
			navigate(`/manga/${id}`);
		} catch {
			// client 已弹错误 toast
		} finally {
			setPendingKey(null);
		}
	}

	function busyKey(m: Manga): string {
		return `${m.sourceId}:${m.key}`;
	}

	function cycle(group: FilterGroup, key: string) {
		setFilters((cur) => {
			const rec = { ...cur[group] };
			const next = nextTri(rec[key] ?? 0);
			if (next === 0) delete rec[key];
			else rec[key] = next;
			return { ...cur, [group]: rec };
		});
	}

	// 无结果源不占 section（其余源还在流式到达，空壳会刷屏）；
	// 全部跑完且一个结果都没有才落全局空态
	const visible = selected.filter((s) => {
		const r = resultsById.get(s.id);
		return !r || r.error !== undefined || r.entries.length > 0;
	});

	return (
		<div className="page container">
			<form
				className="source-page__search"
				onSubmit={(e) => {
					e.preventDefault();
					setQuery(input.trim());
				}}
			>
				<Input
					icon={<Search size={15} />}
					type="search"
					placeholder="搜索所有源…"
					value={input}
					onChange={(e) => setInput(e.currentTarget.value)}
				/>
			</form>

			<div className="filter-header">
				<TriStateDropdown
					label="分级"
					options={CONTENT_RATING_LEVELS.map((l) => ({ key: String(l.value), label: l.label }))}
					states={filters.ratings}
					onCycle={(k) => cycle('ratings', k)}
				/>
				<TriStateDropdown
					label="语言"
					options={langOptions.map((c) => ({ key: c, label: languageLabel(c) }))}
					states={filters.langs}
					onCycle={(k) => cycle('langs', k)}
				/>
				<TriStateDropdown
					label="图源"
					options={sources.map((s) => ({ key: String(s.id), label: s.name }))}
					states={filters.sources}
					onCycle={(k) => cycle('sources', k)}
				/>
			</div>

			{query === '' ? (
				<EmptyState icon={Search} title="跨源搜索" hint="输入关键词，同时搜索所有符合筛选的源" />
			) : selected.length === 0 ? (
				<EmptyState icon={SearchX} title="没有符合筛选的源" hint="调整分级 / 语言 / 图源筛选条件" />
			) : search.isError ? (
				<EmptyState
					icon={TriangleAlert}
					title="加载失败"
					hint={search.error?.message}
					action={
						<Button variant="primary" onClick={() => void search.refetch()}>
							重试
						</Button>
					}
				/>
			) : visible.length === 0 && !search.isFetching ? (
				<EmptyState icon={SearchX} title="没有结果" hint="换个关键词或调整筛选条件试试" />
			) : (
				visible.map((s) => (
					<SourceSection
						key={s.id}
						source={s}
						result={resultsById.get(s.id)}
						pendingKey={pendingKey}
						busyKey={busyKey}
						onOpen={(m) => void openEntry(m)}
						onViewAll={() => navigate(`/browse/${s.id}?q=${encodeURIComponent(query)}`)}
					/>
				))
			)}
		</div>
	);
}

function SourceSection({
	source,
	result,
	pendingKey,
	busyKey,
	onOpen,
	onViewAll
}: {
	source: Source;
	result: SourceSearchResult | undefined;
	pendingKey: string | null;
	busyKey: (m: Manga) => string;
	onOpen: (m: Manga) => void;
	onViewAll: () => void;
}) {
	return (
		<section className="home-section">
			<header className="home-section__head">
				<div className="home-section__titles">
					<h2 className="home-section__title">{source.name}</h2>
				</div>
				<Button size="sm" onClick={onViewAll}>
					查看全部
				</Button>
			</header>
			{!result ? (
				<div className="home-hrow">
					{Array.from({ length: 5 }, (_, i) => (
						<Skeleton key={i} width={120} aspect="2 / 3" style={{ borderRadius: 'var(--r-md)' }} />
					))}
				</div>
			) : result.error ? (
				<p className="search-page__error">{result.error.message}</p>
			) : (
				<div className="home-hrow">
					{result.entries.map((m) => (
						<div className="home-scroller__item" key={m.key}>
							<CoverCard
								src={m.cover ? sourceImageUrl(source.id, m.cover) : m.cover}
								title={m.title}
								subtitle={m.authors?.[0] ?? statusText(m.status)}
								busy={pendingKey === busyKey(m)}
								onClick={() => onOpen(m)}
							/>
						</div>
					))}
				</div>
			)}
		</section>
	);
}
