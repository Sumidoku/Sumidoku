<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import BookOpen from '@lucide/svelte/icons/book-open';
	import ListFilter from '@lucide/svelte/icons/list-filter';
	import Search from '@lucide/svelte/icons/search';
	import SearchX from '@lucide/svelte/icons/search-x';
	import { coverUrl } from '$lib/api/client';
	import { useCategories, useLibrary, useSources } from '$lib/api/hooks';
	import type { LibraryFilter, LibrarySort, TriStateSelection } from '$lib/api/types';
	import { MANGA_CONTENT_RATING_LEVELS, compareSourceName } from '$lib/source';
	import {
		Button,
		Chip,
		CoverCard,
		EmptyState,
		InfiniteList,
		Input,
		Modal,
		PageHeader,
		SegmentedControl,
		Select,
		Skeleton,
		Tabs
	} from '$lib/components';

	const SORT_OPTIONS: { value: LibrarySort; labelKey: string }[] = [
		{ value: 'added', labelKey: 'library.sortAdded' },
		{ value: 'title', labelKey: 'library.sortTitle' },
		{ value: 'lastRead', labelKey: 'library.sortLastRead' },
		{ value: 'unread', labelKey: 'library.sortUnread' }
	];

	const FILTER_OPTIONS: { value: LibraryFilter; labelKey: string }[] = [
		{ value: 'unread', labelKey: 'library.filterUnread' },
		{ value: 'downloaded', labelKey: 'library.filterDownloaded' },
		{ value: 'bookmarked', labelKey: 'library.filterBookmarked' }
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

	let tab = $state(ALL);
	let searchInput = $state('');
	let search = $state('');
	let sort = $state<LibrarySort>('added');
	let filters = $state<LibraryFilter[]>([]);
	let ratings = $state<TriStateSelection>({});
	let sourceTri = $state<TriStateSelection>({});
	let sourceOpen = $state(false);

	// 搜索框防抖 300ms（读取 searchInput 以建立依赖；清理挂起的定时器）
	$effect(() => {
		const v = searchInput;
		const t = setTimeout(() => (search = v.trim()), 300);
		return () => clearTimeout(t);
	});

	const categoryId = $derived(tab === ALL ? undefined : Number(tab));
	const query = useLibrary(
		() => categoryId,
		() => ({
			search: search || undefined,
			sort,
			filter: filters,
			ratings,
			sources: sourceTri
		})
	);
	const categories = useCategories();
	const sourcesQuery = useSources();
	// 停用源仍可能拥有库内条目，候选不去除
	const sourceOptions = $derived([...(sourcesQuery.data ?? [])].sort(compareSourceName));

	const entries = $derived(query.data?.pages.flatMap((p) => p.entries) ?? []);
	const ratingCount = $derived(Object.keys(ratings).length);
	const sourceCount = $derived(Object.keys(sourceTri).length);
	const filtering = $derived(
		search !== '' || filters.length > 0 || ratingCount > 0 || sourceCount > 0
	);

	function toggleFilter(f: LibraryFilter) {
		filters = filters.includes(f) ? filters.filter((x) => x !== f) : [...filters, f];
	}

	function clearFilters() {
		searchInput = '';
		search = '';
		filters = [];
		ratings = {};
		sourceTri = {};
	}

	const tabs = $derived([
		{ key: ALL, label: $_('library.tabAll') },
		{ key: UNCATEGORIZED, label: $_('library.tabUncategorized') },
		...(categories.data ?? []).map((c) => ({
			key: String(c.id),
			label: c.name,
			badge: c.mangaCount
		}))
	]);

	const TRI_OPTIONS = $derived<{ value: 1 | -1 | 0; label: string }[]>([
		{ value: 0, label: $_('library.triNone') },
		{ value: 1, label: $_('library.triInclude') },
		{ value: -1, label: $_('library.triExclude') }
	]);
</script>

<div class="page container">
	<PageHeader title={$_('nav.library')}>
		{#snippet actions()}
			<Input
				class="library__search"
				type="search"
				placeholder={$_('library.searchPlaceholder')}
				bind:value={searchInput}
			>
				{#snippet icon()}<Search size={15} />{/snippet}
			</Input>
			<Select
				aria-label={$_('library.sortLabel')}
				value={sort}
				onchange={(e) => (sort = e.currentTarget.value as LibrarySort)}
			>
				{#each SORT_OPTIONS as o (o.value)}
					<option value={o.value}>{$_(o.labelKey)}</option>
				{/each}
			</Select>
		{/snippet}
	</PageHeader>

	<div class="library__filters">
		{#each FILTER_OPTIONS as o (o.value)}
			<Chip
				color={filters.includes(o.value) ? 'accent' : 'default'}
				onclick={() => toggleFilter(o.value)}
			>
				{$_(o.labelKey)}
			</Chip>
		{/each}
	</div>

	<div class="library__filters" role="group" aria-label={$_('library.filtersGroupLabel')}>
		{#each MANGA_CONTENT_RATING_LEVELS as level (level.value)}
			{@const st = ratings[String(level.value)] ?? 0}
			<Chip
				color={st === 1 ? 'accent' : st === -1 ? 'danger' : 'default'}
				title={$_('library.ratingCycleHint')}
				onclick={() => (ratings = cycled(ratings, String(level.value)))}
			>
				{st === -1 ? '− ' : ''}{level.label}
			</Chip>
		{/each}
		<Button onclick={() => (sourceOpen = true)}>
			<ListFilter size={15} />
			{sourceCount > 0
				? $_('library.sourcesFilterCount', { values: { n: sourceCount } })
				: $_('library.sourcesFilter')}
		</Button>
	</div>

	<div class="library__tabs">
		<Tabs {tabs} active={tab} onChange={(key) => (tab = key)} />
	</div>

	{#if query.isPending}
		<div class="cover-grid">
			{#each Array.from({ length: 12 }) as _item, i (i)}
				<div>
					<Skeleton aspect="2 / 3" class="library__skel-cover" />
					<Skeleton height={14} class="library__skel-line" />
				</div>
			{/each}
		</div>
	{:else if entries.length === 0}
		{#if filtering}
			<EmptyState
				icon={SearchX}
				title={$_('library.emptyFilteredTitle')}
				hint={$_('library.emptyFilteredHint')}
			>
				{#snippet action()}
					<Button onclick={clearFilters}>{$_('library.clearFilters')}</Button>
				{/snippet}
			</EmptyState>
		{:else}
			<EmptyState icon={BookOpen} title={$_('library.emptyTitle')} hint={$_('library.emptyHint')}>
				{#snippet action()}
					<Button variant="primary" onclick={() => goto('/browse')}>
						{$_('library.toBrowse')}
					</Button>
				{/snippet}
			</EmptyState>
		{/if}
	{:else}
		<InfiniteList
			hasNextPage={query.hasNextPage}
			isFetching={query.isFetching}
			onLoadMore={() => void query.fetchNextPage()}
		>
			<div class="cover-grid">
				{#each entries as m (m.id)}
					<CoverCard
						src={coverUrl(m.id, m.cover)}
						title={m.title}
						unreadCount={m.unreadCount}
						to={`/manga/${m.id}`}
					/>
				{/each}
			</div>
		</InfiniteList>
	{/if}

	<Modal open={sourceOpen} title={$_('library.sourcesModalTitle')} onClose={() => (sourceOpen = false)}>
		{#if sourceOptions.length === 0}
			<p class="muted">{$_('library.noSources')}</p>
		{:else}
			<div class="library__source-list">
				{#each sourceOptions as s (s.id)}
					{@const st = sourceTri[String(s.id)] ?? 0}
					<div class="library__source-row">
						<span class="library__source-name">{s.name}</span>
						<SegmentedControl
							options={TRI_OPTIONS}
							value={st}
							onChange={(v) => {
								const next = { ...sourceTri };
								const key = String(s.id);
								if (v === 0) delete next[key];
								else next[key] = v as 1 | -1;
								sourceTri = next;
							}}
						/>
					</div>
				{/each}
			</div>
		{/if}
		{#snippet footer()}
			<Button variant="primary" onclick={() => (sourceOpen = false)}>{$_('library.done')}</Button>
		{/snippet}
	</Modal>
</div>

<style>
	/* 封面位骨架：圆角与行间距对齐 React 版的内联样式 */
	:global(.library__skel-cover) {
		border-radius: var(--r-md);
	}
	:global(.library__skel-line) {
		margin-top: 8px;
	}
</style>
