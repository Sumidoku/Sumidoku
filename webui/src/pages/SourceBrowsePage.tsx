import { useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ArrowLeft, House, ListFilter, Search, SearchX, TriangleAlert } from 'lucide-react';
import {
	useListing,
	useRegisterManga,
	useSource,
	useSourceFilters,
	useSourceListings,
	useSourceSearch
} from '../api/hooks';
import type { Filter, FilterValue, Manga } from '../api/types';
import { statusText } from '../lib/format';
import {
	Button,
	Chip,
	CoverCard,
	EmptyState,
	IconButton,
	InfiniteList,
	Input,
	SegmentedControl,
	Select,
	Skeleton,
	Switch
} from '../components';

type CheckVal = 0 | 1 | -1;

type SortVal = { index: number | null; ascending: boolean };

interface FilterStates {
	textVals: Record<string, string>;
	sortVals: Record<string, SortVal>;
	checkVals: Record<string, CheckVal>;
	selectVals: Record<string, string>;
	multiVals: Record<string, Record<string, CheckVal>>;
	rangeVals: Record<string, { from: string; to: string }>;
}

function initFilterStates(fs: Filter[]): FilterStates {
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
function optionKey(f: Filter, i: number): string {
	return f.ids?.[i] ?? f.options?.[i] ?? '';
}

// 源主页深链（?filters=...）带来的筛选值，解析失败视为没有
function parseFiltersParam(raw: string | null): FilterValue[] {
	if (!raw) return [];
	try {
		const v: unknown = JSON.parse(raw);
		return Array.isArray(v) ? (v as FilterValue[]) : [];
	} catch {
		return [];
	}
}

// 深链筛选值回填面板草稿；filter id 对不上（源改版）的值静默丢弃
function applyFilterValues(s: FilterStates, values: FilterValue[]): FilterStates {
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

export default function SourceBrowsePage() {
	const { sourceId: rawId } = useParams();
	const sourceId = Number(rawId);
	const navigate = useNavigate();

	const source = useSource(sourceId);
	const sourceEnabled = source.data?.enabled ?? true;
	const filtersQuery = useSourceFilters(sourceId, sourceEnabled);
	const listingsQuery = useSourceListings(sourceId, sourceEnabled);

	const filters = useMemo(() => {
		const d = filtersQuery.data;
		return d && d.length > 0 ? d : null;
	}, [filtersQuery.data]);
	const listings = useMemo(() => {
		const d = listingsQuery.data;
		return d && d.length > 0 ? d : null;
	}, [listingsQuery.data]);

	// 深链参数（源主页的 listing/filters 组件）只在进页时消费一次，之后 URL 跟随 mode 切换
	const [searchParams, setSearchParams] = useSearchParams();
	const [initialFilters] = useState(() => parseFiltersParam(searchParams.get('filters')));
	const [mode, setMode] = useState<string>(() => searchParams.get('mode') ?? 'search');
	const [input, setInput] = useState('');
	const [showFilters, setShowFilters] = useState(false);
	const [states, setStates] = useState<FilterStates | null>(null);
	const [submitted, setSubmitted] = useState<{ query: string; filters: FilterValue[] }>({
		query: '',
		filters: initialFilters
	});

	// 筛选默认值只在 filters 首次到达时铺一次，之后的修改都留在面板草稿里
	const initedFor = useRef<number | null>(null);
	useEffect(() => {
		if (!filters || initedFor.current === sourceId) return;
		initedFor.current = sourceId;
		setStates(applyFilterValues(initFilterStates(filters), initialFilters));
	}, [filters, sourceId, initialFilters]);

	function buildFilterValues(): FilterValue[] {
		const out: FilterValue[] = [];
		if (!states) return out;
		for (const f of filters ?? []) {
			if (f.type === 'text') {
				const v = (states.textVals[f.id] ?? '').trim();
				if (v) out.push({ Text: { id: f.id, value: v } });
			} else if (f.type === 'sort') {
				const s = states.sortVals[f.id];
				if (s && s.index !== null) {
					out.push({ Sort: { id: f.id, index: s.index, ascending: s.ascending } });
				}
			} else if (f.type === 'check') {
				const v = states.checkVals[f.id] ?? 0;
				if (v !== 0) out.push({ Check: { id: f.id, value: v } });
			} else if (f.type === 'select') {
				const v = states.selectVals[f.id] ?? '';
				if (v) out.push({ Select: { id: f.id, value: v } });
			} else if (f.type === 'multi-select') {
				const rec = states.multiVals[f.id] ?? {};
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
				const r = states.rangeVals[f.id];
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

	function runSearch() {
		setSubmitted({ query: input.trim(), filters: buildFilterValues() });
	}

	// 输入防抖 500ms 自动触发；筛选草稿不参与防抖，只在回车/应用筛选时一并提交
	const submittedQueryRef = useRef(submitted.query);
	submittedQueryRef.current = submitted.query;
	useEffect(() => {
		const t = setTimeout(() => {
			if (input.trim() !== submittedQueryRef.current) runSearch();
		}, 500);
		return () => clearTimeout(t);
	}, [input]);

	const isSearch = mode === 'search';
	const searchQuery = useSourceSearch(
		sourceId,
		{ query: submitted.query || undefined, filters: submitted.filters },
		isSearch && sourceEnabled
	);
	const listingQuery = useListing(sourceId, mode, !isSearch && sourceEnabled);
	const activeQuery = isSearch ? searchQuery : listingQuery;
	const entries = useMemo(
		() => activeQuery.data?.pages.flatMap((p) => p.entries) ?? [],
		[activeQuery.data]
	);

	function switchMode(m: string) {
		if (m === mode) return;
		setMode(m);
		setSearchParams(m === 'search' ? {} : { mode: m }, { replace: true });
	}

	function cycleMulti(f: Filter, i: number) {
		const key = optionKey(f, i);
		if (!key || !states) return;
		const rec = { ...(states.multiVals[f.id] ?? {}) };
		const cur = rec[key] ?? 0;
		// 可排除时三态循环：未选 → 包含 → 排除 → 未选
		const next: CheckVal = cur === 0 ? 1 : cur === 1 ? (f.can_exclude ? -1 : 0) : 0;
		if (next === 0) delete rec[key];
		else rec[key] = next;
		setStates({ ...states, multiVals: { ...states.multiVals, [f.id]: rec } });
	}

	const register = useRegisterManga();
	const [pendingKey, setPendingKey] = useState<string | null>(null);

	async function openEntry(m: Manga) {
		if (m.id) {
			navigate(`/manga/${m.id}`);
			return;
		}
		if (pendingKey) return;
		setPendingKey(m.key);
		try {
			const { id } = await register.mutateAsync({ sourceId, manga: m });
			navigate(`/manga/${id}`);
		} catch {
			// client 已弹错误 toast
		} finally {
			setPendingKey(null);
		}
	}

	if (source.data && !source.data.enabled) {
		return (
			<div className="page container">
				<div className="source-page__head">
					<IconButton label="返回浏览" onClick={() => navigate('/browse')}>
						<ArrowLeft size={18} />
					</IconButton>
					<h1 className="source-page__title">{source.data.name}</h1>
				</div>
				<EmptyState
					icon={TriangleAlert}
					title="该源已停用"
					hint="重新安装或启用后才能浏览"
				/>
			</div>
		);
	}

	return (
		<div className="page container">
			<div className="source-page__head">
				<IconButton label="返回浏览" onClick={() => navigate('/browse')}>
					<ArrowLeft size={18} />
				</IconButton>
				<h1 className="source-page__title">{source.data?.name ?? '源'}</h1>
				<Button className="source-page__actions" onClick={() => navigate(`/browse/${sourceId}/home`)}>
					<House size={15} />
					主页
				</Button>
			</div>

			{listings && (
				<div className="source-page__modes">
					<SegmentedControl
						options={[
							{ value: 'search', label: '搜索' },
							...listings.map((l) => ({ value: l.id, label: l.name }))
						]}
						value={mode}
						onChange={switchMode}
					/>
				</div>
			)}

			{isSearch && (
				<>
					<form
						className="source-page__search"
						onSubmit={(e) => {
							e.preventDefault();
							runSearch();
						}}
					>
						<Input
							icon={<Search size={15} />}
							type="search"
							placeholder="搜索漫画标题…"
							value={input}
							onChange={(e) => setInput(e.currentTarget.value)}
						/>
						{filters && (
							<Button onClick={() => setShowFilters((v) => !v)}>
								<ListFilter size={15} />
								筛选{showFilters ? ' ▴' : ' ▾'}
							</Button>
						)}
					</form>

					{filters && showFilters && states && (
						<div className="card filter-panel">
							{filters.map((f) => (
								<FilterItem
									key={f.id}
									filter={f}
									states={states}
									setStates={setStates}
									onCycleMulti={cycleMulti}
								/>
							))}
							<div className="filter-panel__footer">
								<Button variant="primary" onClick={runSearch}>
									应用筛选
								</Button>
							</div>
						</div>
					)}
				</>
			)}

			{activeQuery.isPending ? (
				<div className="cover-grid">
					{Array.from({ length: 12 }, (_, i) => (
						<div key={i}>
							<Skeleton aspect="2 / 3" style={{ borderRadius: 'var(--r-md)' }} />
							<Skeleton height={14} style={{ marginTop: 8 }} />
						</div>
					))}
				</div>
			) : activeQuery.isError ? (
				<EmptyState
					icon={TriangleAlert}
					title="加载失败"
					hint={activeQuery.error?.message}
					action={
						<Button variant="primary" onClick={() => void activeQuery.refetch()}>
							重试
						</Button>
					}
				/>
			) : entries.length === 0 ? (
				<EmptyState icon={SearchX} title="没有结果" hint="换个关键词或调整筛选条件试试" />
			) : (
				<InfiniteList
					hasNextPage={activeQuery.hasNextPage}
					isFetching={activeQuery.isFetching}
					onLoadMore={() => void activeQuery.fetchNextPage()}
				>
					<div className="cover-grid">
						{entries.map((m, i) => (
							<CoverCard
								key={m.key + i}
								src={m.cover}
								title={m.title}
								subtitle={m.authors?.[0] ?? statusText(m.status)}
								busy={pendingKey === m.key}
								onClick={() => void openEntry(m)}
							/>
						))}
					</div>
				</InfiniteList>
			)}
		</div>
	);
}

function FilterItem({
	filter: f,
	states,
	setStates,
	onCycleMulti
}: {
	filter: Filter;
	states: FilterStates;
	setStates: (s: FilterStates) => void;
	onCycleMulti: (f: Filter, i: number) => void;
}) {
	if (f.type === 'note') {
		return (
			<div>
				<span className="filter-item__title">{f.title}</span>
				<p className="filter-item__note">{f.text}</p>
			</div>
		);
	}

	if (f.type === 'text') {
		return (
			<div>
				<span className="filter-item__title">{f.title}</span>
				<Input
					type="text"
					placeholder={f.placeholder ?? ''}
					value={states.textVals[f.id] ?? ''}
					onChange={(e) =>
						setStates({ ...states, textVals: { ...states.textVals, [f.id]: e.currentTarget.value } })
					}
				/>
			</div>
		);
	}

	if (f.type === 'sort') {
		const cur = states.sortVals[f.id] ?? { index: null, ascending: true };
		return (
			<div>
				<span className="filter-item__title">{f.title}</span>
				<div className="filter-item__row">
					<Select
						value={cur.index === null ? '' : String(cur.index)}
						onChange={(e) => {
							const v = e.currentTarget.value;
							setStates({
								...states,
								sortVals: {
									...states.sortVals,
									[f.id]: { index: v === '' ? null : Number(v), ascending: cur.ascending }
								}
							});
						}}
					>
						<option value="">默认</option>
						{(f.options ?? []).map((opt, i) => (
							<option key={i} value={i}>
								{opt}
							</option>
						))}
					</Select>
					{f.can_ascend && cur.index !== null && (
						<Button
							onClick={() =>
								setStates({
									...states,
									sortVals: {
										...states.sortVals,
										[f.id]: { ...cur, ascending: !cur.ascending }
									}
								})
							}
						>
							{cur.ascending ? '升序' : '降序'}
						</Button>
					)}
				</div>
			</div>
		);
	}

	if (f.type === 'check') {
		const cur = states.checkVals[f.id] ?? 0;
		if (f.can_exclude) {
			return (
				<div>
					<span className="filter-item__title">
						{f.title}
						{f.name ? ` · ${f.name}` : ''}
					</span>
					<SegmentedControl
						options={[
							{ value: 0, label: '不选' },
							{ value: 1, label: '包含' },
							{ value: -1, label: '排除' }
						]}
						value={cur}
						onChange={(v) => setStates({ ...states, checkVals: { ...states.checkVals, [f.id]: v } })}
					/>
				</div>
			);
		}
		return (
			<div className="filter-item__row" style={{ justifyContent: 'space-between' }}>
				<span className="filter-item__title" style={{ marginBottom: 0 }}>
					{f.title}
					{f.name ? ` · ${f.name}` : ''}
				</span>
				<Switch
					checked={cur === 1}
					onChange={(on) =>
						setStates({ ...states, checkVals: { ...states.checkVals, [f.id]: on ? 1 : 0 } })
					}
					label={f.title ?? undefined}
				/>
			</div>
		);
	}

	if (f.type === 'select') {
		return (
			<div>
				<span className="filter-item__title">{f.title}</span>
				<Select
					value={states.selectVals[f.id] ?? ''}
					onChange={(e) =>
						setStates({ ...states, selectVals: { ...states.selectVals, [f.id]: e.currentTarget.value } })
					}
				>
					<option value="">未选</option>
					{(f.options ?? []).map((opt, i) => (
						<option key={i} value={optionKey(f, i)}>
							{opt}
						</option>
					))}
				</Select>
			</div>
		);
	}

	if (f.type === 'multi-select') {
		return (
			<div>
				<span className="filter-item__title">{f.title}</span>
				<div className="filter-item__chips">
					{(f.options ?? []).map((opt, i) => {
						const st = states.multiVals[f.id]?.[optionKey(f, i)] ?? 0;
						return (
							<Chip
								key={i}
								color={st === 1 ? 'accent' : st === -1 ? 'danger' : 'default'}
								title={f.can_exclude ? '点击切换：包含 / 排除 / 不选' : '点击切换选中'}
								onClick={() => onCycleMulti(f, i)}
							>
								{st === -1 ? '− ' : ''}
								{opt}
							</Chip>
						);
					})}
				</div>
			</div>
		);
	}

	if (f.type === 'range') {
		const r = states.rangeVals[f.id] ?? { from: '', to: '' };
		return (
			<div>
				<span className="filter-item__title">{f.title}</span>
				<div className="filter-item__row">
					<Input
						type="number"
						placeholder={f.min != null ? String(f.min) : '最小'}
						step={f.decimal ? 'any' : '1'}
						value={r.from}
						onChange={(e) =>
							setStates({
								...states,
								rangeVals: { ...states.rangeVals, [f.id]: { ...r, from: e.currentTarget.value } }
							})
						}
					/>
					<span className="faint">—</span>
					<Input
						type="number"
						placeholder={f.max != null ? String(f.max) : '最大'}
						step={f.decimal ? 'any' : '1'}
						value={r.to}
						onChange={(e) =>
							setStates({
								...states,
								rangeVals: { ...states.rangeVals, [f.id]: { ...r, to: e.currentTarget.value } }
							})
						}
					/>
				</div>
			</div>
		);
	}

	return null;
}
