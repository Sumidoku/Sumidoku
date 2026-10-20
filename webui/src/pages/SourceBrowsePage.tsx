import { useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import {
	ArrowLeft,
	Check,
	House,
	Link2,
	ListFilter,
	Search,
	SearchX,
	TriangleAlert,
	X
} from 'lucide-react';
import { coverUrl, sourceImageUrl } from '../api/client';
import {
	useDeepLink,
	useListing,
	useRegisterManga,
	useSource,
	useSourceFilters,
	useSourceListings,
	useSourceSearch
} from '../api/hooks';
import type { Filter, FilterValue, Manga } from '../api/types';
import { statusText } from '../lib/format';
import { loadFilterPrefs, saveFilterPrefs } from '../lib/sourceFilterPrefs';
import {
	Button,
	Chip,
	ChipDropdown,
	Cover,
	CoverCard,
	EmptyState,
	IconButton,
	InfiniteList,
	Input,
	Modal,
	SegmentedControl,
	Select,
	Skeleton,
	Switch,
	pushToast
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

// The selected value takes ids (when present), otherwise the option name
function optionKey(f: Filter, i: number): string {
	return f.ids?.[i] ?? f.options?.[i] ?? '';
}

// Filter values arriving via the source-home deep link (?filters=...); a parse failure counts as none
function parseFiltersParam(raw: string | null): FilterValue[] {
	if (!raw) return [];
	try {
		const v: unknown = JSON.parse(raw);
		return Array.isArray(v) ? (v as FilterValue[]) : [];
	} catch {
		return [];
	}
}

// Deep-link filter values refill the panel draft; values whose filter id no longer matches (source revamped) are silently dropped
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
	const kind = source.data?.kind;
	const isLocal = kind === 'local';
	// Built-in sources (local/opds) have no filters/listings/home/deep-link concept (endpoints 404 unsupported), so those queries are not issued
	const isBuiltin = isLocal || kind === 'opds';
	const filtersQuery = useSourceFilters(sourceId, sourceEnabled && !isBuiltin);
	const listingsQuery = useSourceListings(sourceId, sourceEnabled && !isBuiltin);

	// hidesFiltersWhileSearching: filters hide in search mode, and search requests don't carry filter values
	// (AidokuRunner simply clears filters when the query is non-empty before calling the source)
	const sourceConfig = source.data?.config;

	const filters = useMemo(() => {
		const d = filtersQuery.data;
		return d && d.length > 0 ? d : null;
	}, [filtersQuery.data]);
	const listings = useMemo(() => {
		const d = listingsQuery.data;
		return d && d.length > 0 ? d : null;
	}, [listingsQuery.data]);

	// Deep-link params (from the source home's listing/filters components) are consumed once on entry, then the URL follows mode switches; a deep link beats the locally stored filter values
	const [searchParams, setSearchParams] = useSearchParams();
	const [initialFilters] = useState<FilterValue[]>(() => {
		const deep = parseFiltersParam(searchParams.get('filters'));
		return deep.length > 0 ? deep : loadFilterPrefs(sourceId);
	});
	// ?q= deep link ("view all" on the cross-source search page): prefills the input and enters search mode with that term directly
	const [initialQuery] = useState(() => (searchParams.get('q') ?? '').trim());
	const [mode, setMode] = useState<string>(() => searchParams.get('mode') ?? 'search');
	const [input, setInput] = useState(initialQuery);
	const [showFilters, setShowFilters] = useState(false);
	const [states, setStates] = useState<FilterStates | null>(null);
	const [submitted, setSubmitted] = useState<{ query: string; filters: FilterValue[] }>({
		query: initialQuery,
		filters: initialFilters
	});

	// Filter defaults are laid down only once when filters first arrive; later edits stay in the panel draft
	const initedFor = useRef<number | null>(null);
	useEffect(() => {
		if (!filters || initedFor.current === sourceId) return;
		initedFor.current = sourceId;
		setStates(applyFilterValues(initFilterStates(filters), initialFilters));
	}, [filters, sourceId, initialFilters]);

	function buildFilterValues(draft: FilterStates | null): FilterValue[] {
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

	function runSearch(nextStates?: FilterStates) {
		const query = input.trim();
		// Draft values are always persisted; for sources with hidden filters, a keyword search neither submits them nor overwrites the stored ones
		const values = buildFilterValues(nextStates ?? states);
		const dropFilters = !!sourceConfig?.hidesFiltersWhileSearching && query !== '';
		if (!dropFilters) saveFilterPrefs(sourceId, values);
		setSubmitted({ query, filters: dropFilters ? [] : values });
	}

	// Input auto-submits after a 500ms debounce; the filter draft does not debounce and is only submitted together on Enter/apply-filters
	const submittedQueryRef = useRef(submitted.query);
	submittedQueryRef.current = submitted.query;
	useEffect(() => {
		const t = setTimeout(() => {
			if (input.trim() !== submittedQueryRef.current) runSearch();
		}, 500);
		return () => clearTimeout(t);
	}, [input]);

	const isSearch = mode === 'search';
	const filtersHidden = !!sourceConfig?.hidesFiltersWhileSearching && input.trim() !== '';
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
	// The server normalizes kind to default/list; search mode and unknown kinds (an undeclared listing from a deep link) stay on the grid
	const listMode = !isSearch && listings?.find((l) => l.id === mode)?.kind === 'list';

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
		// When exclusion is allowed the cycle is tri-state: unselected → include → exclude → unselected
		const next = nextMultiVal(cur, !!f.can_exclude);
		if (next === 0) delete rec[key];
		else rec[key] = next;
		setStates({ ...states, multiVals: { ...states.multiVals, [f.id]: rec } });
	}

	const register = useRegisterManga();
	const [pendingKey, setPendingKey] = useState<string | null>(null);

	// Online entries' covers go through the source image proxy; built-in source entries skip cover resolution while browsing (lightweight), registered ones use the manga cover cache
	function entryCover(m: Manga): string | null {
		if (m.cover) return sourceImageUrl(sourceId, m.cover);
		if (isBuiltin && m.id) return coverUrl(m.id);
		return m.cover;
	}

	async function openEntry(m: Manga) {
		if (m.id) {
			navigate(`/manga/${m.id}`);
			return;
		}
		if (pendingKey) return;
		setPendingKey(m.key);
		try {
			const { id } = await register.mutateAsync({ sourceId: m.sourceId ?? sourceId, manga: m });
			navigate(`/manga/${id}`);
		} catch {
			// client already popped an error toast
		} finally {
			setPendingKey(null);
		}
	}

	const deepLink = useDeepLink();
	const [showLink, setShowLink] = useState(false);
	const [linkInput, setLinkInput] = useState('');

	async function openLink() {
		const url = linkInput.trim();
		if (!url || deepLink.isPending) return;
		try {
			const { result } = await deepLink.mutateAsync({ sourceId, url });
			if (!result) {
				pushToast('源未识别该链接', 'info');
				return;
			}
			setShowLink(false);
			setLinkInput('');
			if (result.chapterId != null) {
				navigate(`/reader/${result.chapterId}`);
			} else if (result.mangaId != null) {
				navigate(`/manga/${result.mangaId}`);
			} else if (result.listing) {
				switchMode(result.listing.id);
			}
		} catch {
			// client already popped an error toast
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
				{/* hasHome === false (confirmed no get_home at registration) hides the button; null = unknown for old rows, stays visible. Built-in sources lack this capability */}
				{!isBuiltin && source.data?.hasHome !== false && (
					<Button className="source-page__actions" onClick={() => navigate(`/browse/${sourceId}/home`)}>
						<House size={15} />
						主页
					</Button>
				)}
				{!isBuiltin && (
					<Button onClick={() => setShowLink(true)}>
						<Link2 size={15} />
						打开链接
					</Button>
				)}
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
							placeholder={isLocal ? '搜索本地目录…' : '搜索漫画标题…'}
							value={input}
							onChange={(e) => setInput(e.currentTarget.value)}
						/>
						{filters && !filtersHidden && (
							<Button onClick={() => setShowFilters((v) => !v)}>
								<ListFilter size={15} />
								筛选{showFilters ? ' ▴' : ' ▾'}
							</Button>
						)}
					</form>

					{/* Same as Aidoku's FilterHeaderView: a horizontal filter chip bar under the input; chip changes submit immediately */}
					{filters && !filtersHidden && states && (
						<FilterHeader
							filters={filters}
							states={states}
							setStates={setStates}
							onApply={runSearch}
						/>
					)}

					{filters && !filtersHidden && showFilters && states && (
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
								<Button variant="primary" onClick={() => runSearch()}>
									应用筛选
								</Button>
							</div>
						</div>
					)}
				</>
			)}

			{/* During a streaming search partials already land in the list: skeletons are only for when there is nothing at all, then entries append edge-to-edge */}
			{activeQuery.isPending && entries.length === 0 ? (
				listMode ? (
					<div className="home-rows">
						{Array.from({ length: 8 }, (_, i) => (
							<div key={i} className="home-row">
								<Skeleton width={40} aspect="2 / 3" style={{ borderRadius: 'var(--r-sm)', flex: 'none' }} />
								<span className="home-row__main">
									<Skeleton height={14} width="55%" />
									<Skeleton height={12} width="35%" />
								</span>
							</div>
						))}
					</div>
				) : (
					<div className="cover-grid">
						{Array.from({ length: 12 }, (_, i) => (
							<div key={i}>
								<Skeleton aspect="2 / 3" style={{ borderRadius: 'var(--r-md)' }} />
								<Skeleton height={14} style={{ marginTop: 8 }} />
							</div>
						))}
					</div>
				)
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
					{listMode ? (
						<div className="home-rows">
							{entries.map((m, i) => (
								<button
									key={m.key + i}
									type="button"
									className="home-row"
									disabled={pendingKey === m.key}
									onClick={() => void openEntry(m)}
								>
									<Cover src={entryCover(m)} title={m.title} />
									<span className="home-row__main">
										<span className="home-row__title">{m.title}</span>
										<span className="home-row__subtitle">{m.authors?.[0] ?? statusText(m.status)}</span>
										{m.tags && m.tags.length > 0 && (
											<span className="home-row__tags">{m.tags.join(' · ')}</span>
										)}
									</span>
								</button>
							))}
						</div>
					) : (
						<div className="cover-grid">
							{entries.map((m, i) => (
								<CoverCard
									key={m.key + i}
									src={entryCover(m)}
									title={m.title}
									subtitle={m.authors?.[0] ?? statusText(m.status)}
									busy={pendingKey === m.key}
									onClick={() => void openEntry(m)}
								/>
							))}
						</div>
					)}
				</InfiniteList>
			)}

			<Modal
				open={showLink}
				title="打开源链接"
				onClose={() => setShowLink(false)}
				footer={
					<Button variant="primary" loading={deepLink.isPending} onClick={() => void openLink()}>
						打开
					</Button>
				}
			>
				<form
					onSubmit={(e) => {
						e.preventDefault();
						void openLink();
					}}
				>
					<Input
						type="url"
						icon={<Link2 size={15} />}
						placeholder={`粘贴 ${source.data?.name ?? '源'} 的漫画或章节链接…`}
						value={linkInput}
						onChange={(e) => setLinkInput(e.currentTarget.value)}
					/>
				</form>
			</Modal>
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

// ---- Same as Aidoku's FilterHeaderView: the horizontal filter bar under the search box ----

/** Next state of the tri-state cycle (shared by multi-select chips and the panel) */
function nextMultiVal(cur: CheckVal, canExclude: boolean): CheckVal {
	return cur === 0 ? 1 : cur === 1 ? (canExclude ? -1 : 0) : 0;
}

/** Whether the filter in the draft would produce a submitted value (Aidoku enabledFilters convention, used to enable sorting) */
function isFilterEnabled(f: Filter, states: FilterStates): boolean {
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

interface FilterChipProps {
	filter: Filter;
	states: FilterStates;
	setStates: (s: FilterStates) => void;
	/** Chip changes submit immediately (Aidoku applies filter edits at once); setStates is async, so the new draft is passed along with the call */
	onApply: (s: FilterStates) => void;
}

/** Chip dropdown menu item; children receive close so the menu can be dismissed after a pick */
function ChipMenuItem({
	checked,
	excluded,
	onClick,
	children
}: {
	checked?: boolean;
	excluded?: boolean;
	onClick: () => void;
	children: ReactNode;
}) {
	return (
		<button type="button" className="chip-dropdown__item" onClick={onClick}>
			<span>{children}</span>
			{checked && <Check size={14} />}
			{excluded && <X size={14} />}
		</button>
	);
}

function SortFilterChip({ filter: f, states, setStates, onApply }: FilterChipProps) {
	const options = f.options ?? [];
	const cur = states.sortVals[f.id] ?? { index: null, ascending: true };
	const def = f.default as { index?: number; ascending?: boolean } | null | undefined;
	// Different from the default (unselected counts as null/true) means enabled
	const active =
		cur.index !== null && (cur.index !== (def?.index ?? null) || cur.ascending !== (def?.ascending ?? true));
	const label =
		cur.index !== null && cur.index < options.length
			? `${f.title ? `${f.title}: ` : ''}${options[cur.index]}${f.can_ascend ? (cur.ascending ? ' ↑' : ' ↓') : ''}`
			: (f.title ?? '排序');
	function pick(index: number, close: () => void) {
		// Aidoku: clicking the current item flips direction when ascending is allowed, otherwise selects it and resets to the default direction
		const next: SortVal =
			cur.index === index && f.can_ascend ? { index, ascending: !cur.ascending } : { index, ascending: false };
		const nextStates = { ...states, sortVals: { ...states.sortVals, [f.id]: next } };
		setStates(nextStates);
		onApply(nextStates);
		close();
	}
	return (
		<ChipDropdown active={active} label={label}>
			{(close) =>
				options.map((opt, i) => (
					<ChipMenuItem key={i} checked={cur.index === i} onClick={() => pick(i, close)}>
						{opt}
						{cur.index === i && f.can_ascend ? (cur.ascending ? ' ↑' : ' ↓') : ''}
					</ChipMenuItem>
				))
			}
		</ChipDropdown>
	);
}

function CheckFilterChip({ filter: f, states, setStates, onApply }: FilterChipProps) {
	const cur = states.checkVals[f.id] ?? 0;
	return (
		<Chip
			color={cur !== 0 ? 'accent' : 'default'}
			onClick={() => {
				const nextStates = {
					...states,
					checkVals: { ...states.checkVals, [f.id]: nextMultiVal(cur, !!f.can_exclude) }
				};
				setStates(nextStates);
				onApply(nextStates);
			}}
		>
			{f.can_exclude && cur === 1 && <Check size={13} />}
			{f.can_exclude && cur === -1 && <X size={13} />}
			{f.name ?? f.title ?? ''}
		</Chip>
	);
}

function SelectFilterChip({ filter: f, states, setStates, onApply }: FilterChipProps) {
	const cur = states.selectVals[f.id] ?? '';
	const defVal = typeof f.default === 'string' ? f.default : '';
	const active = cur !== '' && cur !== defVal;
	function pick(value: string, close: () => void) {
		const nextStates = { ...states, selectVals: { ...states.selectVals, [f.id]: value } };
		setStates(nextStates);
		onApply(nextStates);
		close();
	}
	return (
		<ChipDropdown active={active} label={f.title ?? ''}>
			{(close) => (
				<>
					{cur !== '' && (
						<ChipMenuItem checked onClick={() => pick('', close)}>
							未选
						</ChipMenuItem>
					)}
					{(f.options ?? []).map((opt, i) => {
						const key = optionKey(f, i);
						return (
							<ChipMenuItem key={i} checked={cur === key} onClick={() => pick(key, close)}>
								{opt}
							</ChipMenuItem>
						);
					})}
				</>
			)}
		</ChipDropdown>
	);
}

function MultiSelectFilterChip({ filter: f, states, setStates, onApply }: FilterChipProps) {
	const rec = { ...(states.multiVals[f.id] ?? {}) };
	const count = Object.keys(rec).length;
	function toggle(i: number) {
		const key = optionKey(f, i);
		if (!key) return;
		const next = nextMultiVal(rec[key] ?? 0, !!f.can_exclude);
		if (next === 0) delete rec[key];
		else rec[key] = next;
		const nextStates = { ...states, multiVals: { ...states.multiVals, [f.id]: rec } };
		setStates(nextStates);
		onApply(nextStates);
		// Aidoku menuActionDismissDisabled: the menu stays open for consecutive multi-selecting
	}
	return (
		<ChipDropdown active={count > 0} label={`${f.title ?? ''}${count > 0 ? `（${count}）` : ''}`}>
			{() =>
				(f.options ?? []).map((opt, i) => {
					const key = optionKey(f, i);
					const st = rec[key] ?? 0;
					return (
						<ChipMenuItem key={i} checked={st === 1} excluded={st === -1} onClick={() => toggle(i)}>
							{st === -1 ? '− ' : ''}
							{opt}
						</ChipMenuItem>
					);
				})
			}
		</ChipDropdown>
	);
}

/** Horizontal filter chip bar under the search box: one chip per filter, enabled ones first */
function FilterHeader({
	filters,
	states,
	setStates,
	onApply
}: {
	filters: Filter[];
	states: FilterStates;
	setStates: (s: FilterStates) => void;
	onApply: (s: FilterStates) => void;
}) {
	const sorted = [...filters].sort(
		(a, b) => Number(isFilterEnabled(b, states)) - Number(isFilterEnabled(a, states))
	);
	return (
		<div className="filter-header">
			{sorted.map((f) => {
				if (f.hide_from_header || f.type === 'note' || f.type === 'text' || f.type === 'range') return null;
				const props = { filter: f, states, setStates, onApply };
				if (f.type === 'sort') return <SortFilterChip key={f.id} {...props} />;
				// Aidoku: a check filter with a default does not enter the header (after a reset it would not wrongly show as enabled)
				if (f.type === 'check') {
					if (f.default != null) return null;
					return <CheckFilterChip key={f.id} {...props} />;
				}
				if (f.type === 'select') return <SelectFilterChip key={f.id} {...props} />;
				if (f.type === 'multi-select') return <MultiSelectFilterChip key={f.id} {...props} />;
				return null;
			})}
		</div>
	);
}
