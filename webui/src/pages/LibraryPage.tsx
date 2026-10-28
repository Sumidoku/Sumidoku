import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { BookOpen, SearchX } from 'lucide-react';
import { api, coverUrl } from '../api/client';
import {
	qk,
	useBatchChapters,
	useCategories,
	useDeleteDownloads,
	useEnqueueDownloads,
	useFilterGroups,
	useLibrary,
	useLibraryAuthors,
	useLibrarySourceCounts,
	useLibraryTags,
	useServerSettings,
	useSources
} from '../api/hooks';
import type { LibrarySort } from '../api/types';
import type { TriStateSelection } from '../lib/triState';
import { compareSourceName } from '../lib/source';
import { downloadEligibleIds } from '../lib/librarySelection';
import { hasLibraryFilterPrefs, loadLibraryFilters, saveLibraryFilters } from '../lib/libraryFilterPrefs';
import { BatchActionBar, Button, CoverCard, EmptyState, InfiniteList, PageHeader, Skeleton, pushToast } from '../components';
import { LibraryToolbar, UNCATEGORIZED } from './library/LibraryToolbar';
import { ChangeCategoriesModal, RemoveFromLibraryModal } from './library/LibraryBatchModals';
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

export interface LibraryPageProps {
	/** Seeds selection mode with the loaded prefix preselected; SSR tests cannot click their way in */
	initialSelecting?: boolean;
}

export default function LibraryPage({ initialSelecting = false }: LibraryPageProps) {
	const { t } = useTranslation();
	const qc = useQueryClient();
	// Controls restore from localStorage; the prefs are loaded once — the pruning effect below drops ids that no longer exist
	const [initialPrefs] = useState(loadLibraryFilters);
	const [searchInput, setSearchInput] = useState(initialPrefs.search);
	const [search, setSearch] = useState(initialPrefs.search.trim());
	const [sort, setSort] = useState<LibrarySort>(initialPrefs.sort);
	const [filterTri, setFilterTri] = useState<TriStateSelection>(initialPrefs.filters);
	const [ratings, setRatings] = useState<TriStateSelection>(initialPrefs.ratings);
	const [sourceTri, setSourceTri] = useState<TriStateSelection>(initialPrefs.sources);
	const [categoryTri, setCategoryTri] = useState<TriStateSelection>(initialPrefs.categories);
	const [authorTri, setAuthorTri] = useState<TriStateSelection>(initialPrefs.authors);
	const [tagTri, setTagTri] = useState<TriStateSelection>(initialPrefs.tags);
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
		authors: authorTri,
		tags: tagTri,
		groupId: groupId ?? undefined
	});
	const categories = useCategories();
	const filterGroups = useFilterGroups();
	const sourcesQuery = useSources();
	const countsQuery = useLibrarySourceCounts();
	const authorsQuery = useLibraryAuthors();
	const tagsQuery = useLibraryTags();
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
		saveLibraryFilters({
			search: searchInput,
			sort,
			filters: filterTri,
			ratings,
			sources: sourceTri,
			categories: categoryTri,
			authors: authorTri,
			tags: tagTri,
			group: groupId
		});
	}, [searchInput, sort, filterTri, ratings, sourceTri, categoryTri, authorTri, tagTri, groupId]);

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
	// Same staleness rule for restored author/tag names: a name no longer present in the library
	// would filter invisibly, so prune each selection once its name list has loaded
	useEffect(() => {
		if (authorsQuery.data) {
			setAuthorTri((cur) => pruneSelection(cur, new Set(authorsQuery.data.map((a) => a.name))));
		}
		if (tagsQuery.data) {
			setTagTri((cur) => pruneSelection(cur, new Set(tagsQuery.data.map((t) => t.name))));
		}
	}, [authorsQuery.data, tagsQuery.data]);
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
	// The author/tag endpoints return the entry counts directly and arrive pre-sorted case-insensitively,
	// so the labels can always carry the same `name（count）` shape the source dropdown uses once counted
	const authorOptions = useMemo(
		() => (authorsQuery.data ?? []).map((a) => ({ key: a.name, label: `${a.name}（${a.count}）` })),
		[authorsQuery.data]
	);
	const tagOptions = useMemo(
		() => (tagsQuery.data ?? []).map((tag) => ({ key: tag.name, label: `${tag.name}（${tag.count}）` })),
		[tagsQuery.data]
	);

	const entries = useMemo(
		() => query.data?.pages.flatMap((p) => p.entries) ?? [],
		[query.data]
	);

	const [selecting, setSelecting] = useState(initialSelecting);
	// Entering selection mode selects the whole loaded prefix, so the seeded state must start the same way
	const [selected, setSelected] = useState<Set<number>>(() =>
		initialSelecting ? new Set(entries.map((m) => m.id)) : new Set()
	);
	const [catOpen, setCatOpen] = useState(false);
	const [removeOpen, setRemoveOpen] = useState(false);
	const [loopBusy, setLoopBusy] = useState(false);

	// Selection is derived against the live list, so entries a refetch dropped never linger in the count
	const selectedEntries = useMemo(() => entries.filter((m) => selected.has(m.id)), [entries, selected]);
	const allSelected = entries.length > 0 && entries.every((m) => selected.has(m.id));
	const localSourceIds = useMemo(
		() => new Set((sourcesQuery.data ?? []).filter((s) => s.kind === 'local').map((s) => s.id)),
		[sourcesQuery.data]
	);
	const downloadIds = useMemo(
		() => downloadEligibleIds(selectedEntries, localSourceIds),
		[selectedEntries, localSourceIds]
	);

	const batchChapters = useBatchChapters();
	const enqueueDownloads = useEnqueueDownloads();
	const deleteDownloads = useDeleteDownloads();
	const batchBusy =
		batchChapters.isPending || enqueueDownloads.isPending || deleteDownloads.isPending || loopBusy;

	// A refetch that empties the list (entries removed elsewhere) must not strand the selection chrome
	useEffect(() => {
		if (selecting && entries.length === 0) exitSelect();
	}, [selecting, entries.length]);

	function enterSelect() {
		setSelected(new Set(entries.map((m) => m.id)));
		setSelecting(true);
	}

	function exitSelect() {
		setSelecting(false);
		setSelected(new Set());
	}

	function toggleSelected(id: number) {
		setSelected((prev) => {
			const next = new Set(prev);
			if (next.has(id)) next.delete(id);
			else next.add(id);
			return next;
		});
	}

	function toggleAll() {
		setSelected(allSelected ? new Set() : new Set(entries.map((m) => m.id)));
	}

	/** Every batch op can move unread/download badges, category counts and detail caches of the touched manga */
	function invalidateSelected(ids: number[]) {
		void qc.invalidateQueries({ queryKey: qk.library });
		void qc.invalidateQueries({ queryKey: qk.categories });
		for (const id of ids) {
			void qc.invalidateQueries({ queryKey: qk.manga(id) });
			void qc.invalidateQueries({ queryKey: qk.chapters(id) });
		}
	}

	function markSelected(isRead: boolean) {
		const ids = selectedEntries.map((m) => m.id);
		batchChapters.mutate(
			{ mangaIds: ids, isRead },
			{
				onSuccess: (res) => {
					pushToast(
						isRead
							? t('library.markedRead', { count: res.updated })
							: t('library.markedUnread', { count: res.updated }),
						'info'
					);
					invalidateSelected(ids);
					exitSelect();
				}
			}
		);
	}

	function downloadSelected() {
		enqueueDownloads.mutate(
			{ mangaIds: downloadIds },
			{
				onSuccess: (res) => {
					pushToast(t('library.queuedChapters', { count: res.queued.length }), 'info');
					invalidateSelected(downloadIds);
					exitSelect();
				}
			}
		);
	}

	function deleteSelectedDownloads() {
		const ids = selectedEntries.map((m) => m.id);
		deleteDownloads.mutate(ids, {
			onSuccess: (res) => {
				pushToast(t('library.deletedDownloads', { count: res.deleted }), 'info');
				invalidateSelected(ids);
				exitSelect();
			}
		});
	}

	async function applyCategories(categoryIds: number[]) {
		const ids = selectedEntries.map((m) => m.id);
		setLoopBusy(true);
		try {
			// There is no batch endpoint: the same final set is PUT per manga, one toast for the whole batch
			await Promise.all(
				ids.map((id) => api(`/manga/${id}/categories`, { method: 'PUT', body: { categoryIds }, silent: true }))
			);
			pushToast(t('library.catsSaved'), 'info');
			setCatOpen(false);
			invalidateSelected(ids);
			exitSelect();
		} catch {
			pushToast(t('library.batchFailed'), 'error');
		} finally {
			setLoopBusy(false);
		}
	}

	async function confirmRemove() {
		const ids = selectedEntries.map((m) => m.id);
		setLoopBusy(true);
		try {
			await Promise.all(ids.map((id) => api(`/library/${id}`, { method: 'DELETE', silent: true })));
			pushToast(t('library.removedSelected', { count: ids.length }), 'info');
			setRemoveOpen(false);
			invalidateSelected(ids);
			exitSelect();
		} catch {
			pushToast(t('library.batchFailed'), 'error');
		} finally {
			setLoopBusy(false);
		}
	}

	const filtering =
		search !== '' ||
		groupId !== null ||
		Object.keys(filterTri).length > 0 ||
		Object.keys(ratings).length > 0 ||
		Object.keys(sourceTri).length > 0 ||
		Object.keys(categoryTri).length > 0 ||
		Object.keys(authorTri).length > 0 ||
		Object.keys(tagTri).length > 0;

	function clearFilters() {
		setSearchInput('');
		setSearch('');
		setFilterTri({});
		setRatings({});
		setSourceTri({});
		setCategoryTri({});
		setAuthorTri({});
		setTagTri({});
		setGroupId(null);
	}

	/** Cycling a category chip by hand counts as a user choice, so the first-visit default must not override it later */
	function cycleCategory(key: string) {
		defaultSeed.touched = true;
		setCategoryTri((cur) => cycled(cur, key));
	}

	return (
		<div className="page container">
			<PageHeader
				title={t('common.nav.library')}
				actions={
					selecting ? (
						<>
							<Button size="sm" onClick={toggleAll}>
								{allSelected ? t('library.deselectAll') : t('library.selectAll')}
							</Button>
							<span className="library-sel-count">
								{t('library.selectedCount', { count: selectedEntries.length })}
							</span>
							<Button size="sm" onClick={exitSelect}>
								{t('library.done')}
							</Button>
						</>
					) : (
						<Button size="sm" disabled={entries.length === 0} onClick={enterSelect}>
							{t('library.select')}
						</Button>
					)
				}
			/>
			{!selecting && (
				<LibraryToolbar
					searchInput={searchInput}
					onSearchInput={setSearchInput}
					sort={sort}
					onSort={setSort}
					filterTri={filterTri}
					ratings={ratings}
					sourceTri={sourceTri}
					categoryTri={categoryTri}
					authorTri={authorTri}
					tagTri={tagTri}
					groupId={groupId}
					groups={filterGroups.data ?? []}
					categories={categories.data ?? []}
					sourceOptions={triOptions}
					authorOptions={authorOptions}
					tagOptions={tagOptions}
					showUncategorized={showUncategorized}
					onFilterCycle={(key) => setFilterTri((cur) => cycled(cur, key))}
					onRatingCycle={(key) => setRatings((cur) => cycled(cur, key))}
					onSourceCycle={(key) => setSourceTri((cur) => cycled(cur, key))}
					onCategoryCycle={cycleCategory}
					onAuthorCycle={(key) => setAuthorTri((cur) => cycled(cur, key))}
					onTagCycle={(key) => setTagTri((cur) => cycled(cur, key))}
					onGroupChange={setGroupId}
					onResetRatings={() => setRatings({})}
					onResetSources={() => setSourceTri({})}
					onResetAuthors={() => setAuthorTri({})}
					onResetTags={() => setTagTri({})}
					onClearFilters={clearFilters}
				/>
			)}

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
					<div className={selecting ? 'cover-grid cover-grid--selecting' : 'cover-grid'}>
						{entries.map((m) => (
							<CoverCard
								key={m.id}
								src={coverUrl(m.id, m.cover)}
								title={m.title}
								unreadCount={m.unreadCount}
								trackerProviders={m.trackerProviders}
								trashed={m.trashed}
								to={selecting ? undefined : `/manga/${m.id}`}
								selecting={selecting}
								selected={selected.has(m.id)}
								onToggle={() => toggleSelected(m.id)}
							/>
						))}
					</div>
				</InfiniteList>
			)}

			{selecting && (
				<BatchActionBar count={t('library.selectedShort', { count: selectedEntries.length })}>
					<Button
						size="sm"
						variant="primary"
						disabled={downloadIds.length === 0 || batchBusy}
						title={downloadIds.length < selectedEntries.length ? t('library.downloadSkipHint') : undefined}
						onClick={downloadSelected}
					>
						{t('library.download')}
					</Button>
					<Button size="sm" disabled={selectedEntries.length === 0 || batchBusy} onClick={deleteSelectedDownloads}>
						{t('library.deleteDownloads')}
					</Button>
					<Button
						size="sm"
						disabled={selectedEntries.length === 0 || batchBusy}
						onClick={() => markSelected(true)}
					>
						{t('library.markRead')}
					</Button>
					<Button
						size="sm"
						disabled={selectedEntries.length === 0 || batchBusy}
						onClick={() => markSelected(false)}
					>
						{t('library.markUnread')}
					</Button>
					<Button
						size="sm"
						disabled={selectedEntries.length === 0 || batchBusy}
						onClick={() => setCatOpen(true)}
					>
						{t('library.changeCategories')}
					</Button>
					<Button
						size="sm"
						variant="danger"
						disabled={selectedEntries.length === 0 || batchBusy}
						onClick={() => setRemoveOpen(true)}
					>
						{t('library.removeFromLibrary')}
					</Button>
				</BatchActionBar>
			)}

			<ChangeCategoriesModal
				open={catOpen}
				count={selectedEntries.length}
				categories={categories.data ?? []}
				busy={loopBusy}
				onClose={() => setCatOpen(false)}
				onApply={(ids) => void applyCategories(ids)}
			/>
			<RemoveFromLibraryModal
				open={removeOpen}
				count={selectedEntries.length}
				busy={loopBusy}
				onClose={() => setRemoveOpen(false)}
				onConfirm={() => void confirmRemove()}
			/>
		</div>
	);
}
