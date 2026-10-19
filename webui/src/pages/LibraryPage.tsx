import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { BookOpen, Search, SearchX } from 'lucide-react';
import { coverUrl } from '../api/client';
import { useCategories, useLibrary, useSources } from '../api/hooks';
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
	Tabs,
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

const ALL = 'all';
const UNCATEGORIZED = '0';

/** 三态循环的下一态：未选 → 包含 → 排除 → 未选 */
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

export default function LibraryPage() {
	const [tab, setTab] = useState(ALL);
	const [searchInput, setSearchInput] = useState('');
	const [search, setSearch] = useState('');
	const [sort, setSort] = useState<LibrarySort>('added');
	const [filters, setFilters] = useState<LibraryFilter[]>([]);
	const [ratings, setRatings] = useState<TriStateSelection>({});
	const [sourceTri, setSourceTri] = useState<TriStateSelection>({});

	useEffect(() => {
		const t = setTimeout(() => setSearch(searchInput.trim()), 300);
		return () => clearTimeout(t);
	}, [searchInput]);

	const categoryId = tab === ALL ? undefined : Number(tab);
	const query = useLibrary(categoryId, {
		search: search || undefined,
		sort,
		filter: filters,
		ratings,
		sources: sourceTri
	});
	const categories = useCategories();
	const sourcesQuery = useSources();
	// 停用源仍可能拥有库内条目，候选不去除
	const sourceOptions = useMemo(
		() => [...(sourcesQuery.data ?? [])].sort(compareSourceName),
		[sourcesQuery.data]
	);

	const entries = useMemo(
		() => query.data?.pages.flatMap((p) => p.entries) ?? [],
		[query.data]
	);
	const ratingCount = Object.keys(ratings).length;
	const sourceCount = Object.keys(sourceTri).length;
	const filtering =
		search !== '' || filters.length > 0 || ratingCount > 0 || sourceCount > 0;

	function toggleFilter(f: LibraryFilter) {
		setFilters((cur) => (cur.includes(f) ? cur.filter((x) => x !== f) : [...cur, f]));
	}

	function clearFilters() {
		setSearchInput('');
		setSearch('');
		setFilters([]);
		setRatings({});
		setSourceTri({});
	}

	const tabs = [
		{ key: ALL, label: '全部' },
		{ key: UNCATEGORIZED, label: '未分类' },
		...(categories.data ?? []).map((c) => ({
			key: String(c.id),
			label: c.name,
			badge: c.mangaCount
		}))
	];

	return (
		<div className="page container">
			<PageHeader
				title="书库"
				actions={
					<>
						<Input
							className="library__search"
							icon={<Search size={15} />}
							type="search"
							placeholder="搜索书库…"
							value={searchInput}
							onChange={(e) => setSearchInput(e.currentTarget.value)}
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
					</>
				}
			/>

			<div className="library__filters">
				{FILTER_OPTIONS.map((o) => (
					<Chip
						key={o.value}
						color={filters.includes(o.value) ? 'accent' : 'default'}
						onClick={() => toggleFilter(o.value)}
					>
						{o.label}
					</Chip>
				))}
			</div>

			<div className="library__filters" role="group" aria-label="内容分级与图源">
				{MANGA_CONTENT_RATING_LEVELS.map((level) => {
					const st = ratings[String(level.value)] ?? 0;
					return (
						<Chip
							key={level.value}
							color={st === 1 ? 'accent' : st === -1 ? 'danger' : 'default'}
							title="点击切换：包含 / 排除 / 不选"
							onClick={() => setRatings((cur) => cycled(cur, String(level.value)))}
						>
							{st === -1 ? '− ' : ''}
							{level.label}
						</Chip>
					);
				})}
				{/* 与跨源搜索页同款的图源三态下拉（含停用源：库内条目可能属于它们） */}
				<TriStateDropdown
					label="图源"
					options={sourceOptions.map((s) => ({ key: String(s.id), label: s.name }))}
					states={sourceTri}
					onCycle={(k) => setSourceTri((cur) => cycled(cur, k))}
				/>
			</div>

			<div className="library__tabs">
				<Tabs tabs={tabs} active={tab} onChange={setTab} />
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
