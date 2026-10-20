<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import Search from '@lucide/svelte/icons/search';
	import SearchX from '@lucide/svelte/icons/search-x';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { useGlobalSearch, useRegisterManga, useSources } from '$lib/api/hooks';
	import type { Manga, Source, SourceSearchResult } from '$lib/api/types';
	import {
		type GlobalSearchFilterPrefs,
		type TriState,
		loadGlobalSearchFilters,
		saveGlobalSearchFilters
	} from '$lib/globalSearchPrefs';
	import {
		CONTENT_RATING_LEVELS,
		MULTI_LANGUAGE,
		compareLanguageLabel,
		compareSourceName,
		isMultiLanguage,
		languageLabel
	} from '$lib/source';
	import { Button, EmptyState, Input } from '$lib/components';
	import SourceSection from './SourceSection.svelte';
	import TriDropdown from './TriDropdown.svelte';

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

	const sourcesQuery = useSources();

	let filters = $state<GlobalSearchFilterPrefs>(loadGlobalSearchFilters());
	$effect(() => saveGlobalSearchFilters(filters));

	// 输入防抖 500ms，与源内搜索一致
	let input = $state('');
	let query = $state('');
	$effect(() => {
		const v = input;
		const t = setTimeout(() => (query = v.trim()), 500);
		return () => clearTimeout(t);
	});

	// 停用源不参与也不进筛选候选；展示与候选顺序按源名（浏览页同口径）
	const sources = $derived(
		(sourcesQuery.data ?? []).filter((s) => s.enabled).sort(compareSourceName)
	);
	const langOptions = $derived.by(() => {
		const set = new Set<string>();
		for (const s of sources) {
			for (const l of s.languages) {
				const code = l.trim().toLowerCase();
				if (code && !isMultiLanguage(code)) set.add(code);
			}
		}
		// 与 Aidoku 一致：多语言哨兵恒在列表首位
		return [MULTI_LANGUAGE, ...[...set].sort(compareLanguageLabel)];
	});

	const selected = $derived(sources.filter((s) => sourceMatches(s, filters)));
	const sourceIds = $derived(selected.map((s) => s.id));

	const search = useGlobalSearch(
		() => sourceIds,
		() => query,
		() => query !== '' && sourceIds.length > 0
	);
	const resultsById = $derived.by(() => {
		const map = new Map<number, SourceSearchResult>();
		for (const r of search.data?.sources ?? []) map.set(r.sourceId, r);
		return map;
	});

	const register = useRegisterManga();
	// busy 态用 `<sourceId>:<key>` 复合键：不同源可能撞同 key，单 key 会互相误标
	let pendingKey = $state<string | null>(null);

	async function openEntry(m: Manga) {
		if (m.id) {
			goto(`/manga/${m.id}`);
			return;
		}
		// 聚合结果的条目都带着自己的 sourceId，登记必须落到它的属主源上
		const owner = m.sourceId;
		if (owner === undefined || pendingKey) return;
		pendingKey = `${owner}:${m.key}`;
		try {
			const { id } = await register.mutateAsync({ sourceId: owner, manga: m });
			goto(`/manga/${id}`);
		} catch {
			// client 已弹错误 toast
		} finally {
			pendingKey = null;
		}
	}

	function cycle(group: FilterGroup, key: string) {
		const rec = { ...filters[group] };
		const next = nextTri(rec[key] ?? 0);
		if (next === 0) delete rec[key];
		else rec[key] = next;
		filters = { ...filters, [group]: rec };
	}

	// 无结果源不占 section（其余源还在流式到达，空壳会刷屏）；
	// 全部跑完且一个结果都没有才落全局空态
	const visible = $derived(
		selected.filter((s) => {
			const r = resultsById.get(s.id);
			return !r || r.error !== undefined || r.entries.length > 0;
		})
	);

	function onSubmit(e: SubmitEvent) {
		e.preventDefault();
		query = input.trim();
	}
</script>

<div class="page container">
	<form class="source-page__search" onsubmit={onSubmit}>
		<Input type="search" placeholder={$_('search.placeholder')} bind:value={input}>
			{#snippet icon()}<Search size={15} />{/snippet}
		</Input>
	</form>

	<div class="filter-header">
		<TriDropdown
			label={$_('search.filterRatings')}
			options={CONTENT_RATING_LEVELS.map((l) => ({ key: String(l.value), label: l.label }))}
			states={filters.ratings}
			onCycle={(k) => cycle('ratings', k)}
		/>
		<TriDropdown
			label={$_('search.filterLangs')}
			options={langOptions.map((c) => ({ key: c, label: languageLabel(c) }))}
			states={filters.langs}
			onCycle={(k) => cycle('langs', k)}
		/>
		<TriDropdown
			label={$_('search.filterSources')}
			options={sources.map((s) => ({ key: String(s.id), label: s.name }))}
			states={filters.sources}
			onCycle={(k) => cycle('sources', k)}
		/>
	</div>

	{#if query === ''}
		<EmptyState icon={Search} title={$_('search.emptyTitle')} hint={$_('search.emptyHint')} />
	{:else if selected.length === 0}
		<EmptyState
			icon={SearchX}
			title={$_('search.noSourcesTitle')}
			hint={$_('search.noSourcesHint')}
		/>
	{:else if search.isError}
		<EmptyState icon={TriangleAlert} title={$_('search.loadFailedTitle')} hint={search.error?.message}>
			{#snippet action()}
				<Button variant="primary" onclick={() => void search.refetch()}>{$_('common.retry')}</Button>
			{/snippet}
		</EmptyState>
	{:else if visible.length === 0 && !search.isFetching}
		<EmptyState
			icon={SearchX}
			title={$_('search.noResultsTitle')}
			hint={$_('search.noResultsHint')}
		/>
	{:else}
		{#each visible as s (s.id)}
			<SourceSection
				source={s}
				result={resultsById.get(s.id)}
				{pendingKey}
				onOpen={(m) => void openEntry(m)}
				onViewAll={() => goto(`/browse/${s.id}?q=${encodeURIComponent(query)}`)}
			/>
		{/each}
	{/if}
</div>
