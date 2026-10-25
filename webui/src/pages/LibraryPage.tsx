import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { BookOpen, SearchX } from 'lucide-react';
import { coverUrl } from '../api/client';
import { useCategories, useFilterGroups, useLibrary, useLibrarySourceCounts, useServerSettings, useSources } from '../api/hooks';
import type { LibrarySort } from '../api/types';
import type { TriStateSelection } from '../lib/triState';
import { compareSourceName } from '../lib/source';
import { hasLibraryFilterPrefs, loadLibraryFilters, saveLibraryFilters } from '../lib/libraryFilterPrefs';
import { Button, CoverCard, EmptyState, InfiniteList, PageHeader, Skeleton } from '../components';
import { LibraryToolbar, UNCATEGORIZED } from './library/LibraryToolbar';
import './library/library.css';

function nextTri(cur: 1 | -1 | 0): 1 | -1 | 0 {
	return cur === 0 ? 1 : cur === 1 ? -1 : 0;
}

function cycled(sel: TriStateSelection, key: string): TriStateSelection {
	const next = { ...sel };
	const v = nextTri(next[key] ?? 0);
	if (v === 0) delete next[key];
	else next[key] = v;
	return next;
}

/** Drops entries whose key is not in `valid` (and not one of `extras`); returns the original object when nothing changed */
function pruneSelection(
	sel: TriStateSelection,
	valid: ReadonlySet<string>,
	...extras: string[]
): TriStateSelection {
	let changed = false;
	const next: TriStateSelection = {};
	for (const [k, v] of Object.entries(sel)) {
		if (valid.has(k) || extras.includes(k)) next[k] = v;
		else changed = true;
	}
	return changed ? next : sel;
}

export default function LibraryPage() {
	const { t } = useTranslation();
	// Controls restore from localStorage; the prefs are loaded once — the pruning effect below drops ids that no longer exist
	const [initialPrefs] = useState(loadLibraryFilters);
	const [searchInput, setSearchInput] = useState(initialPrefs.search);
	const [search, setSearch] = useState(initialPrefs.search.trim());
	const [sort, setSort] = useState<LibrarySort>(initialPrefs.sort);
	const [filterTri, setFilterTri] = useState<TriStateSelection>(initialPrefs.filters);
	const [ratings, setRatings] = useState<TriStateSelection>(initialPrefs.ratings);
	const [sourceTri, setSourceTri] = useState<TriStateSelection>(initialPrefs.sources);
	const [categoryTri, setCategoryTri] = useState<TriStateSelection>(initialPrefs.categories);
	const [groupId, setGroupId] = useState<number | null>(initialPrefs.group);
	// library.default_category only seeds the very first visit: once any chip state exists in
	// localStorage the user's own selection always wins (#243), and a manual chip change before the
	// settings arrive suppresses the seed too
	const [defaultSeed] = useState(() => ({ stored: hasLibraryFilterPrefs(), applied: false, touched: false }));

	useEffect(() => {
		const timer = setTimeout(() => setSearch(searchInput.trim()), 300);
		return () => clearTimeout(timer);
	}, [searchInput]);

	const query = useLibrary({
		search: search || undefined,
		sort,
		filters: filterTri,
		ratings,
		sources: sourceTri,
		categories: categoryTri,
		groupId: groupId ?? undefined
	});
	const categories = useCategories();
	const filterGroups = useFilterGroups();
	const sourcesQuery = useSources();
	const countsQuery = useLibrarySourceCounts();
	const serverSettings = useServerSettings();
	const showUncategorized = serverSettings.data?.library.showUncategorized !== false;

	// First-visit seed of the initial category chip from the server setting; runs at most once, and
	// never over a stored selection or a chip the user already touched
	useEffect(() => {
		const raw = serverSettings.data?.library.defaultCategory;
		if (defaultSeed.stored || defaultSeed.applied || defaultSeed.touched || raw === undefined) return;
		defaultSeed.applied = true;
		if (raw === 'uncategorized') setCategoryTri({ [UNCATEGORIZED]: 1 });
		else if (raw !== 'all' && /^\d+$/.test(String(raw))) setCategoryTri({ [String(raw)]: 1 });
	}, [serverSettings.data, defaultSeed]);

	// Persist all library controls across sessions (the clear-filters button and chip clicks flow through the same setters)
	useEffect(() => {
		saveLibraryFilters({ search: searchInput, sort, filters: filterTri, ratings, sources: sourceTri, categories: categoryTri, group: groupId });
	}, [searchInput, sort, filterTri, ratings, sourceTri, categoryTri, groupId]);

	// Restored ids may belong to since-deleted categories or uninstalled sources: a stale include id
	// would filter the library down to nothing with no chip showing why, so drop unknown ids once
	// both lists are loaded (ratings keys are the fixed 1..=3 set and filters the fixed flag vocabulary)
	useEffect(() => {
		if (!categories.data || !sourcesQuery.data) return;
		setCategoryTri((cur) =>
			pruneSelection(
				cur,
				new Set((categories.data ?? []).map((c) => String(c.id))),
				...(showUncategorized ? [UNCATEGORIZED] : [])
			)
		);
		setSourceTri((cur) => pruneSelection(cur, new Set(sourcesQuery.data?.map((s) => String(s.id)))));
	}, [categories.data, sourcesQuery.data, showUncategorized]);
	// A selected group that was since deleted must not keep filtering invisibly
	useEffect(() => {
		if (filterGroups.data && groupId !== null && !filterGroups.data.some((g) => g.id === groupId)) {
			setGroupId(null);
		}
	}, [filterGroups.data, groupId]);
	const counts = useMemo(() => {
		const map = new Map<number, number>();
		for (const c of countsQuery.data ?? []) map.set(c.sourceId, c.count);
		return map;
	}, [countsQuery.data]);
	// A disabled source may still own library entries, so candidates are not removed
	const sourceOptions = useMemo(
		() => [...(sourcesQuery.data ?? [])].sort(compareSourceName),
		[sourcesQuery.data]
	);
	// Candidates of the source dropdown: once counts arrive, sources with 0 library entries hide automatically
	// (ones currently tri-state-selected stay, to avoid a dangling filter); before counts arrive the full list renders to avoid flicker
	const counted = countsQuery.data !== undefined;
	const triOptions = sourceOptions
		.filter((s) => !counted || (counts.get(s.id) ?? 0) > 0 || sourceTri[String(s.id)])
		.map((s) => ({
			key: String(s.id),
			label: counted ? `${s.name}（${counts.get(s.id) ?? 0}）` : s.name
		}));

	const entries = useMemo(
		() => query.data?.pages.flatMap((p) => p.entries) ?? [],
		[query.data]
	);
	const filtering =
		search !== '' ||
		groupId !== null ||
		Object.keys(filterTri).length > 0 ||
		Object.keys(ratings).length > 0 ||
		Object.keys(sourceTri).length > 0 ||
		Object.keys(categoryTri).length > 0;

	function clearFilters() {
		setSearchInput('');
		setSearch('');
		setFilterTri({});
		setRatings({});
		setSourceTri({});
		setCategoryTri({});
		setGroupId(null);
	}

	/** Cycling a category chip by hand counts as a user choice, so the first-visit default must not override it later */
	function cycleCategory(key: string) {
		defaultSeed.touched = true;
		setCategoryTri((cur) => cycled(cur, key));
	}

	return (
		<div className="page container">
			<PageHeader title={t('common.nav.library')} />
			<LibraryToolbar
				searchInput={searchInput}
				onSearchInput={setSearchInput}
				sort={sort}
				onSort={setSort}
				filterTri={filterTri}
				ratings={ratings}
				sourceTri={sourceTri}
				categoryTri={categoryTri}
				groupId={groupId}
				groups={filterGroups.data ?? []}
				categories={categories.data ?? []}
				sourceOptions={triOptions}
				showUncategorized={showUncategorized}
				onFilterCycle={(key) => setFilterTri((cur) => cycled(cur, key))}
				onRatingCycle={(key) => setRatings((cur) => cycled(cur, key))}
				onSourceCycle={(key) => setSourceTri((cur) => cycled(cur, key))}
				onCategoryCycle={cycleCategory}
				onGroupChange={setGroupId}
				onResetRatings={() => setRatings({})}
				onResetSources={() => setSourceTri({})}
				onClearFilters={clearFilters}
			/>

			{query.isPending ? (
				<div className="cover-grid">
					{Array.from({ length: 12 }, (_, i) => (
						<div key={i}>
							<Skeleton aspect="2 / 3" style={{ borderRadius: 'var(--r-md)' }} />
							<Skeleton height={14} style={{ marginTop: 8 }} />
						</div>
					))}
				</div>
			) : entries.length === 0 ? (
				filtering ? (
					<EmptyState
						icon={SearchX}
						title={t('library.emptyFilteredTitle')}
						hint={t('library.emptyFilteredHint')}
						action={<Button onClick={clearFilters}>{t('library.clearFilters')}</Button>}
					/>
				) : (
					<EmptyState
						icon={BookOpen}
						title={t('library.emptyTitle')}
						hint={t('library.emptyHint')}
						action={
							<Link to="/browse">
								<Button variant="primary">{t('library.goBrowse')}</Button>
							</Link>
						}
					/>
				)
			) : (
				<InfiniteList
					hasNextPage={query.hasNextPage}
					isFetching={query.isFetching}
					onLoadMore={() => void query.fetchNextPage()}
				>
					<div className="cover-grid">
						{entries.map((m) => (
							<CoverCard
								key={m.id}
								src={coverUrl(m.id, m.cover)}
								title={m.title}
								unreadCount={m.unreadCount}
								trashed={m.trashed}
								to={`/manga/${m.id}`}
							/>
						))}
					</div>
				</InfiniteList>
			)}
		</div>
	);
}
