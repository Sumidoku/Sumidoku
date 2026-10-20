import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { BookOpen, Search, SearchX } from 'lucide-react';
import { coverUrl } from '../api/client';
import { useCategories, useLibrary, useLibrarySourceCounts, useSources } from '../api/hooks';
import type { LibraryFilter, LibrarySort, TriStateSelection } from '../api/types';
import { MANGA_CONTENT_RATING_LEVELS, compareSourceName } from '../lib/source';
import {
	Button,
	Chip,
	CoverCard,
	EmptyState,
	InfiniteList,
	Input,
	PageHeader,
	Select,
	Skeleton,
	TriStateDropdown
} from '../components';

const SORT_OPTIONS: { value: LibrarySort; label: string }[] = [
	{ value: 'added', label: '最近入库' },
	{ value: 'title', label: '标题' },
	{ value: 'lastRead', label: '最近阅读' },
	{ value: 'unread', label: '未读最多' }
];

const FILTER_OPTIONS: { value: LibraryFilter; label: string }[] = [
	{ value: 'unread', label: '未读' },
	{ value: 'downloaded', label: '已下载' },
	{ value: 'bookmarked', label: '书签' }
];

/** Tri-state key of the built-in "uncategorized" arm (mirrors the API's category=0) */
const UNCATEGORIZED = '0';
/** Next state of the tri-state cycle: unselected → include → exclude → unselected */
function nextTri(cur: 1 | -1 | 0): 1 | -1 | 0 {
	return cur === 0 ? 1 : cur === 1 ? -1 : 0;
}

/** Same tri-state chip tooltip as the manga detail page's chapter filter */
function triTitle(label: string, t: 1 | -1 | 0): string {
	if (t === 1) return `当前只显示有${label}的漫画，点击改为排除`;
	if (t === -1) return `当前排除有${label}的漫画，点击关闭筛选`;
	return `当前不筛选${label}，点击改为只显示`;
}

function cycled(sel: TriStateSelection, key: string): TriStateSelection {
	const next = { ...sel };
	const v = nextTri(next[key] ?? 0);
	if (v === 0) delete next[key];
	else next[key] = v;
	return next;
}

export default function LibraryPage() {
	const [searchInput, setSearchInput] = useState('');
	const [search, setSearch] = useState('');
	const [sort, setSort] = useState<LibrarySort>('added');
	const [filterTri, setFilterTri] = useState<TriStateSelection>({});
	const [ratings, setRatings] = useState<TriStateSelection>({});
	const [sourceTri, setSourceTri] = useState<TriStateSelection>({});
	const [categoryTri, setCategoryTri] = useState<TriStateSelection>({});

	useEffect(() => {
		const t = setTimeout(() => setSearch(searchInput.trim()), 300);
		return () => clearTimeout(t);
	}, [searchInput]);

	const query = useLibrary({
		search: search || undefined,
		sort,
		filters: filterTri,
		ratings,
		sources: sourceTri,
		categories: categoryTri
	});
	const categories = useCategories();
	const sourcesQuery = useSources();
	const countsQuery = useLibrarySourceCounts();
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
	}

	return (
		<div className="page container">
			<PageHeader
				title="书库"
				actions={
					<Input
						className="library__search"
						icon={<Search size={15} />}
						type="search"
						placeholder="搜索书库…"
						value={searchInput}
						onChange={(e) => setSearchInput(e.currentTarget.value)}
					/>
				}
			/>

			<div className="library__filters">
				{/* Category tri-state chips replace the old category tabs: no chip selected = 全部; 未分类 is the API's category=0 arm */}
				<Chip
					color={
						categoryTri[UNCATEGORIZED] === 1
							? 'accent'
							: categoryTri[UNCATEGORIZED] === -1
								? 'danger'
								: 'default'
					}
					className={categoryTri[UNCATEGORIZED] === -1 ? 'chip--exclude' : undefined}
					title={triTitle('未分类', categoryTri[UNCATEGORIZED] ?? 0)}
					onClick={() => setCategoryTri((cur) => cycled(cur, UNCATEGORIZED))}
				>
					{categoryTri[UNCATEGORIZED] === -1 ? '− ' : ''}未分类
				</Chip>
				{(categories.data ?? []).map((c) => {
					const st = categoryTri[String(c.id)] ?? 0;
					return (
						<Chip
							key={c.id}
							color={st === 1 ? 'accent' : st === -1 ? 'danger' : 'default'}
							className={st === -1 ? 'chip--exclude' : undefined}
							title={triTitle(c.name, st)}
							onClick={() => setCategoryTri((cur) => cycled(cur, String(c.id)))}
						>
							{st === -1 ? '− ' : ''}
							{c.name}（{c.mangaCount}）
						</Chip>
					);
				})}
				{/* Same tri-state dropdown as the cross-source search page; sources include disabled ones — library entries may belong to them */}
				<TriStateDropdown
					label="分级"
					options={MANGA_CONTENT_RATING_LEVELS.map((l) => ({ key: String(l.value), label: l.label }))}
					states={ratings}
					onCycle={(k) => setRatings((cur) => cycled(cur, k))}
				/>
				<TriStateDropdown
					label="图源"
					options={triOptions}
					states={sourceTri}
					onCycle={(k) => setSourceTri((cur) => cycled(cur, k))}
				/>
				<Select
					aria-label="排序"
					value={sort}
					onChange={(e) => setSort(e.currentTarget.value as LibrarySort)}
				>
					{SORT_OPTIONS.map((o) => (
						<option key={o.value} value={o.value}>
							{o.label}
						</option>
					))}
				</Select>
				{/* Same tri-state chips as the manga detail page's chapter filter: unselected → include → exclude */}
				{FILTER_OPTIONS.map((o) => {
					const st = filterTri[o.value] ?? 0;
					return (
						<Chip
							key={o.value}
							color={st === 1 ? 'accent' : st === -1 ? 'danger' : 'default'}
							className={st === -1 ? 'chip--exclude' : undefined}
							title={triTitle(o.label, st)}
							onClick={() => setFilterTri((cur) => cycled(cur, o.value))}
						>
							{st === -1 ? '− ' : ''}
							{o.label}
						</Chip>
					);
				})}
			</div>

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
						title="没有符合条件的漫画"
						hint="换个关键词，或者清空筛选条件再看看"
						action={<Button onClick={clearFilters}>清空筛选</Button>}
					/>
				) : (
					<EmptyState
						icon={BookOpen}
						title="书架还是空的"
						hint="去「浏览」里挑几本漫画加入书库吧"
						action={
							<Link to="/browse">
								<Button variant="primary">去浏览</Button>
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
								to={`/manga/${m.id}`}
							/>
						))}
					</div>
				</InfiniteList>
			)}

		</div>
	);
}
