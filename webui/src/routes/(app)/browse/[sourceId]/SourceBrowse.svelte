<script lang="ts">
	import { untrack } from 'svelte';
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import House from '@lucide/svelte/icons/house';
	import Link2 from '@lucide/svelte/icons/link-2';
	import ListFilter from '@lucide/svelte/icons/list-filter';
	import Search from '@lucide/svelte/icons/search';
	import SearchX from '@lucide/svelte/icons/search-x';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { sourceImageUrl } from '$lib/api/client';
	import {
		useDeepLink,
		useListing,
		useRegisterManga,
		useSource,
		useSourceFilters,
		useSourceListings,
		useSourceSearch
	} from '$lib/api/hooks';
	import type { Filter, FilterValue, Manga } from '$lib/api/types';
	import { statusText } from '$lib/format';
	import { loadFilterPrefs, saveFilterPrefs } from '$lib/sourceFilterPrefs';
	import {
		Button,
		Cover,
		CoverCard,
		EmptyState,
		IconButton,
		InfiniteList,
		Input,
		Modal,
		SegmentedControl,
		Skeleton,
		pushToast
	} from '$lib/components';
	import {
		applyFilterValues,
		buildFilterValues,
		initFilterStates,
		nextMultiVal,
		optionKey,
		parseFiltersParam,
		type FilterStates
	} from './filterState';
	import FilterHeader from './FilterHeader.svelte';
	import FilterItem from './FilterItem.svelte';

	let { sourceId }: { sourceId: number } = $props();

	// 深链参数（源主页的 listing/filters 组件、跨源搜索页的「查看全部」）只在进页时消费一次
	const sp = page.url.searchParams;
	const deep = parseFiltersParam(sp.get('filters'));
	// 有深链时优先于本机存的筛选值
	const initialFilters: FilterValue[] = deep.length > 0 ? deep : loadFilterPrefs(untrack(() => sourceId));
	// ?q= 深链：预填输入框并直接以该词进搜索态
	const initialQuery = (sp.get('q') ?? '').trim();

	const source = useSource(() => sourceId);
	const sourceEnabled = $derived(source.data?.enabled ?? true);
	const filtersQuery = useSourceFilters(() => sourceId, () => sourceEnabled);
	const listingsQuery = useSourceListings(() => sourceId, () => sourceEnabled);

	// hidesFiltersWhileSearching：搜索态藏起过滤器，且搜索请求不带过滤器值
	const sourceConfig = $derived(source.data?.config);

	const filters = $derived.by(() => {
		const d = filtersQuery.data;
		return d && d.length > 0 ? d : null;
	});
	const listings = $derived.by(() => {
		const d = listingsQuery.data;
		return d && d.length > 0 ? d : null;
	});

	let mode = $state(sp.get('mode') ?? 'search');
	let input = $state(initialQuery);
	let showFilters = $state(false);
	let states = $state<FilterStates | null>(null);
	let submitted = $state<{ query: string; filters: FilterValue[] }>({
		query: initialQuery,
		filters: initialFilters
	});

	const setStates = (s: FilterStates) => {
		states = s;
	};

	// 筛选默认值只在 filters 首次到达时铺一次，之后的修改都留在面板草稿里
	let inited = false;
	$effect(() => {
		const f = filters;
		if (!f || inited) return;
		inited = true;
		states = applyFilterValues(initFilterStates(f), initialFilters);
	});

	function runSearch(nextStates?: FilterStates) {
		const query = input.trim();
		// 草稿值始终持久化；隐藏筛选的源在带关键词搜索时不提交、也不覆盖存储
		const values = buildFilterValues(filters, nextStates ?? states);
		const dropFilters = !!sourceConfig?.hidesFiltersWhileSearching && query !== '';
		if (!dropFilters) saveFilterPrefs(sourceId, values);
		submitted = { query, filters: dropFilters ? [] : values };
	}

	// 输入防抖 500ms 自动触发；筛选草稿不参与防抖，只在回车/应用筛选时一并提交
	$effect(() => {
		const current = input;
		const t = setTimeout(() => {
			if (current.trim() !== submitted.query) runSearch();
		}, 500);
		return () => clearTimeout(t);
	});

	const isSearch = $derived(mode === 'search');
	const filtersHidden = $derived(!!sourceConfig?.hidesFiltersWhileSearching && input.trim() !== '');

	const searchQuery = useSourceSearch(
		() => sourceId,
		() => ({ query: submitted.query || undefined, filters: submitted.filters }),
		() => isSearch && sourceEnabled
	);
	const listingQuery = useListing(() => sourceId, () => mode, () => !isSearch && sourceEnabled);
	const activeQuery = $derived(isSearch ? searchQuery : listingQuery);
	const entries = $derived(activeQuery.data?.pages.flatMap((p) => p.entries) ?? []);
	// 服务端把 kind 归一成 default/list；搜索态与 kind 未知维持网格
	const listMode = $derived(!isSearch && listings?.find((l) => l.id === mode)?.kind === 'list');

	function switchMode(m: string) {
		if (m === mode) return;
		mode = m;
		void goto(m === 'search' ? `/browse/${sourceId}` : `/browse/${sourceId}?mode=${encodeURIComponent(m)}`, {
			replaceState: true,
			// SvelteKit 3：reset:false 同时保留滚动位置与焦点（取代旧的 noScroll+keepFocus）
			reset: false
		});
	}

	function cycleMulti(f: Filter, i: number) {
		const key = optionKey(f, i);
		if (!key || !states) return;
		const rec = { ...(states.multiVals[f.id] ?? {}) };
		const cur = rec[key] ?? 0;
		// 可排除时三态循环：未选 → 包含 → 排除 → 未选
		const next = nextMultiVal(cur, !!f.can_exclude);
		if (next === 0) delete rec[key];
		else rec[key] = next;
		states = { ...states, multiVals: { ...states.multiVals, [f.id]: rec } };
	}

	const register = useRegisterManga();
	let pendingKey = $state<string | null>(null);

	async function openEntry(m: Manga) {
		if (m.id) {
			void goto(`/manga/${m.id}`);
			return;
		}
		if (pendingKey) return;
		pendingKey = m.key;
		try {
			const { id } = await register.mutateAsync({ sourceId: m.sourceId ?? sourceId, manga: m });
			void goto(`/manga/${id}`);
		} catch {
			// client 已弹错误 toast
		} finally {
			pendingKey = null;
		}
	}

	const deepLink = useDeepLink();
	let showLink = $state(false);
	let linkInput = $state('');

	async function openLink() {
		const url = linkInput.trim();
		if (!url || deepLink.isPending) return;
		try {
			const { result } = await deepLink.mutateAsync({ sourceId, url });
			if (!result) {
				pushToast($_('sourceBrowse.linkNotRecognized'), 'info');
				return;
			}
			showLink = false;
			linkInput = '';
			if (result.chapterId != null) {
				void goto(`/reader/${result.chapterId}`);
			} else if (result.mangaId != null) {
				void goto(`/manga/${result.mangaId}`);
			} else if (result.listing) {
				switchMode(result.listing.id);
			}
		} catch {
			// client 已弹错误 toast
		}
	}
</script>

{#if source.data && !source.data.enabled}
	<div class="page container">
		<div class="source-page__head">
			<IconButton label={$_('sourceBrowse.backLabel')} onclick={() => goto('/browse')}>
				<ArrowLeft size={18} />
			</IconButton>
			<h1 class="source-page__title">{source.data.name}</h1>
		</div>
		<EmptyState
			icon={TriangleAlert}
			title={$_('sourceBrowse.disabledTitle')}
			hint={$_('sourceBrowse.disabledHint')}
		/>
	</div>
{:else}
	<div class="page container">
		<div class="source-page__head">
			<IconButton label={$_('sourceBrowse.backLabel')} onclick={() => goto('/browse')}>
				<ArrowLeft size={18} />
			</IconButton>
			<h1 class="source-page__title">{source.data?.name ?? $_('sourceBrowse.sourceFallback')}</h1>
			<Button class="source-page__actions" onclick={() => goto(`/browse/${sourceId}/home`)}>
				<House size={15} />
				{$_('sourceBrowse.home')}
			</Button>
			<Button onclick={() => (showLink = true)}>
				<Link2 size={15} />
				{$_('sourceBrowse.openLink')}
			</Button>
		</div>

		{#if listings}
			<div class="source-page__modes">
				<SegmentedControl
					options={[
						{ value: 'search', label: $_('sourceBrowse.searchTab') },
						...listings.map((l) => ({ value: l.id, label: l.name }))
					]}
					value={mode}
					onChange={switchMode}
				/>
			</div>
		{/if}

		{#if isSearch}
			<form
				class="source-page__search"
				onsubmit={(e) => {
					e.preventDefault();
					runSearch();
				}}
			>
				<Input
					type="search"
					placeholder={$_('sourceBrowse.searchPlaceholder')}
					bind:value={input}
				>
					{#snippet icon()}<Search size={15} />{/snippet}
				</Input>
				{#if filters && !filtersHidden}
					<Button onclick={() => (showFilters = !showFilters)}>
						<ListFilter size={15} />
						{$_('sourceBrowse.filters')}{showFilters ? ' ▴' : ' ▾'}
					</Button>
				{/if}
			</form>

			<!-- Aidoku FilterHeaderView 同款：输入框下方的横向筛选 chip 条；chip 改动即提交 -->
			{#if filters && !filtersHidden && states}
				<FilterHeader {filters} {states} {setStates} onApply={runSearch} />
			{/if}

			{#if filters && !filtersHidden && showFilters && states}
				<div class="card filter-panel">
					{#each filters as f (f.id)}
						<FilterItem filter={f} {states} {setStates} onCycleMulti={cycleMulti} />
					{/each}
					<div class="filter-panel__footer">
						<Button variant="primary" onclick={() => runSearch()}>
							{$_('sourceBrowse.applyFilters')}
						</Button>
					</div>
				</div>
			{/if}
		{/if}

		<!-- 流式搜索中 partial 已进列表：骨架只留给一条都没有时，之后边到边追加 -->
		{#if activeQuery.isPending && entries.length === 0}
			{#if listMode}
				<div class="home-rows">
					{#each Array.from({ length: 8 }) as _row, i (i)}
						<div class="home-row">
							<Skeleton width={40} aspect="2 / 3" class="skel-thumb" />
							<span class="home-row__main">
								<Skeleton height={14} width="55%" />
								<Skeleton height={12} width="35%" />
							</span>
						</div>
					{/each}
				</div>
			{:else}
				<div class="cover-grid">
					{#each Array.from({ length: 12 }) as _cell, i (i)}
						<div>
							<Skeleton aspect="2 / 3" class="skel-cover" />
							<Skeleton height={14} class="skel-title" />
						</div>
					{/each}
				</div>
			{/if}
		{:else if activeQuery.isError}
			<EmptyState
				icon={TriangleAlert}
				title={$_('sourceBrowse.loadFailed')}
				hint={activeQuery.error?.message}
			>
				{#snippet action()}
					<Button variant="primary" onclick={() => void activeQuery.refetch()}>
						{$_('common.retry')}
					</Button>
				{/snippet}
			</EmptyState>
		{:else if entries.length === 0}
			<EmptyState
				icon={SearchX}
				title={$_('sourceBrowse.noResults')}
				hint={$_('sourceBrowse.noResultsHint')}
			/>
		{:else}
			<InfiniteList
				hasNextPage={activeQuery.hasNextPage}
				isFetching={activeQuery.isFetching}
				onLoadMore={() => void activeQuery.fetchNextPage()}
			>
				{#if listMode}
					<div class="home-rows">
						{#each entries as m, i (m.key + '-' + i)}
							<button
								type="button"
								class="home-row"
								disabled={pendingKey === m.key}
								onclick={() => void openEntry(m)}
							>
								<Cover src={m.cover ? sourceImageUrl(sourceId, m.cover) : m.cover} title={m.title} />
								<span class="home-row__main">
									<span class="home-row__title">{m.title}</span>
									<span class="home-row__subtitle">{m.authors?.[0] ?? statusText(m.status)}</span>
									{#if m.tags && m.tags.length > 0}
										<span class="home-row__tags">{m.tags.join(' · ')}</span>
									{/if}
								</span>
							</button>
						{/each}
					</div>
				{:else}
					<div class="cover-grid">
						{#each entries as m, i (m.key + '-' + i)}
							<CoverCard
								src={m.cover ? sourceImageUrl(sourceId, m.cover) : m.cover}
								title={m.title}
								subtitle={m.authors?.[0] ?? statusText(m.status)}
								busy={pendingKey === m.key}
								onclick={() => void openEntry(m)}
							/>
						{/each}
					</div>
				{/if}
			</InfiniteList>
		{/if}

		<Modal
			open={showLink}
			title={$_('sourceBrowse.openLinkTitle')}
			onClose={() => (showLink = false)}
		>
			<form
				onsubmit={(e) => {
					e.preventDefault();
					void openLink();
				}}
			>
				<Input
					type="url"
					placeholder={$_('sourceBrowse.linkPlaceholder', {
						values: { name: source.data?.name ?? $_('sourceBrowse.sourceFallback') }
					})}
					bind:value={linkInput}
				>
					{#snippet icon()}<Link2 size={15} />{/snippet}
				</Input>
			</form>
			{#snippet footer()}
				<Button variant="primary" loading={deepLink.isPending} onclick={() => void openLink()}>
					{$_('sourceBrowse.open')}
				</Button>
			{/snippet}
		</Modal>
	</div>
{/if}

<style>
	:global(.skel-cover) {
		border-radius: var(--r-md);
	}
	:global(.skel-title) {
		margin-top: 8px;
	}
	:global(.skel-thumb) {
		flex: none;
	}
</style>
