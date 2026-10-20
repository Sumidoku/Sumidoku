<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import House from '@lucide/svelte/icons/house';
	import Search from '@lucide/svelte/icons/search';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { sourceImageUrl } from '$lib/api/client';
	import { useRegisterManga, useSource, useSourceHome } from '$lib/api/hooks';
	import type {
		HomeComponent,
		HomeComponentValue,
		HomeFilterItem,
		HomeLink,
		Listing,
		Manga,
		MangaWithChapter
	} from '$lib/api/types';
	import { chapterLabel, relativeTime } from '$lib/format';
	import { Button, Chip, Cover, CoverCard, EmptyState, IconButton, Skeleton } from '$lib/components';
	import HomeImageScroller from './HomeImageScroller.svelte';

	const sourceId = $derived(Number(page.params.sourceId));

	const source = useSource(() => sourceId);
	const homeQuery = useSourceHome(
		() => sourceId,
		() => source.data?.enabled ?? true
	);

	const register = useRegisterManga();
	let pendingKey = $state<string | null>(null);

	async function openManga(m: Manga) {
		if (m.id) {
			goto(`/manga/${m.id}`);
			return;
		}
		if (pendingKey) return;
		pendingKey = m.key;
		try {
			const { id } = await register.mutateAsync({ sourceId, manga: m });
			goto(`/manga/${id}`);
		} catch {
			// client 已弹错误 toast
		} finally {
			pendingKey = null;
		}
	}

	function openListing(listing: Listing) {
		goto(`/browse/${sourceId}?mode=${encodeURIComponent(listing.id)}`);
	}

	function openLink(link: HomeLink) {
		const v = link.value;
		if (!v) return;
		if ('Url' in v) {
			window.open(v.Url, '_blank', 'noopener,noreferrer');
		} else if ('Listing' in v) {
			openListing(v.Listing);
		} else {
			void openManga(v.Manga);
		}
	}

	function openFilterItem(item: HomeFilterItem) {
		const qs = new URLSearchParams({ mode: 'search' });
		if (item.values && item.values.length > 0) {
			qs.set('filters', JSON.stringify(item.values));
		}
		goto(`/browse/${sourceId}?${qs}`);
	}

	function linkBusy(link: HomeLink): boolean {
		return link.value !== null && 'Manga' in link.value && pendingKey === link.value.Manga.key;
	}

	// 防盗链要的头只有服务端能按源声明带上，封面统一走服务端代理
	function proxyImage(url: string | null): string | null {
		return url ? sourceImageUrl(sourceId, url) : url;
	}

	// 变体取值器：'X' in v 收窄到对应成员，返回其载荷或 null（空数组/对象仍为真）
	const getImageScroller = (v: HomeComponentValue) => ('ImageScroller' in v ? v.ImageScroller : null);
	const getBigScroller = (v: HomeComponentValue) => ('BigScroller' in v ? v.BigScroller : null);
	const getScroller = (v: HomeComponentValue) => ('Scroller' in v ? v.Scroller : null);
	const getMangaList = (v: HomeComponentValue) => ('MangaList' in v ? v.MangaList : null);
	const getMangaChapterList = (v: HomeComponentValue) =>
		'MangaChapterList' in v ? v.MangaChapterList : null;
	const getFilters = (v: HomeComponentValue) => ('Filters' in v ? v.Filters : null);
	const getLinks = (v: HomeComponentValue) => ('Links' in v ? v.Links : null);

	function sectionListing(c: HomeComponent): Listing | null {
		const v = c.value;
		if ('Scroller' in v) return v.Scroller.listing;
		if ('MangaList' in v) return v.MangaList.listing;
		if ('MangaChapterList' in v) return v.MangaChapterList.listing;
		return null;
	}

	// 流式期间组件还没填实的判定：layout 槽位的 payload 各变体条目数组为空
	function isPendingComponent(c: HomeComponent): boolean {
		const v = c.value;
		if ('ImageScroller' in v) return v.ImageScroller.links.length === 0;
		if ('BigScroller' in v) return v.BigScroller.entries.length === 0;
		if ('Scroller' in v) return v.Scroller.entries.length === 0;
		if ('MangaList' in v) return v.MangaList.entries.length === 0;
		if ('MangaChapterList' in v) return v.MangaChapterList.entries.length === 0;
		if ('Filters' in v) return v.Filters.length === 0;
		return v.Links.length === 0;
	}

	function chunk<T>(items: T[], size: number): T[][] {
		const out: T[][] = [];
		for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
		return out;
	}

	const home = $derived(homeQuery.data);
	// 流式加载中 partial 已写进缓存：有数据就边到边渲染，骨架只留给全无数据时
	const showSkeleton = $derived(
		(!home && homeQuery.isPending) ||
			(home != null && home.components.length === 0 && homeQuery.isFetching)
	);
	// layout 事件先到（槽位带真实标题、payload 为空），component 事件填实；流式期间未填实的槽位渲染骨架
	const streaming = $derived(homeQuery.isFetching);
</script>

{#snippet mangaRow(link: HomeLink, index: number, ranking: boolean)}
	{@const tags = link.value && 'Manga' in link.value ? link.value.Manga.tags : null}
	<button type="button" class="home-row" disabled={linkBusy(link)} onclick={() => openLink(link)}>
		{#if ranking}<span class="home-row__rank">{index + 1}</span>{/if}
		<Cover src={proxyImage(link.image_url)} title={link.title} />
		<span class="home-row__main">
			<span class="home-row__title">{link.title}</span>
			{#if link.subtitle}<span class="home-row__subtitle">{link.subtitle}</span>{/if}
			{#if tags && tags.length > 0}<span class="home-row__tags">{tags.join(' · ')}</span>{/if}
		</span>
		<ChevronRight size={16} class="home-row__chevron" />
	</button>
{/snippet}

{#snippet chapterRow(mwc: MangaWithChapter)}
	{@const manga = mwc.manga}
	{@const chapter = mwc.chapter}
	{@const rel = relativeTime(chapter.date_uploaded)}
	<button
		type="button"
		class="home-row"
		disabled={pendingKey === manga.key}
		onclick={() => void openManga(manga)}
	>
		<Cover src={proxyImage(manga.cover)} title={manga.title} />
		<span class="home-row__main">
			<span class="home-row__title">{manga.title}</span>
			<span class="home-row__subtitle">{chapterLabel(chapter)}{rel ? ` · ${rel}` : ''}</span>
		</span>
		<ChevronRight size={16} class="home-row__chevron" />
	</button>
{/snippet}

{#snippet pendingBody(c: HomeComponent)}
	{@const v = c.value}
	{#if getMangaList(v) || getMangaChapterList(v) || getLinks(v)}
		<div class="home-rows">
			{#each [0, 1, 2] as i (i)}<Skeleton height={64} class="home-skel-rounded" />{/each}
		</div>
	{:else if getFilters(v)}
		<div class="home-chips">
			{#each [0, 1, 2, 3] as i (i)}<Skeleton width={72} height={26} class="home-skel-chip" />{/each}
		</div>
	{:else if getImageScroller(v)}
		{@const imgScr = getImageScroller(v)!}
		{@const ratio =
			imgScr.width && imgScr.height && imgScr.width > 0 && imgScr.height > 0
				? `${imgScr.width} / ${imgScr.height}`
				: '16 / 9'}
		<div class="home-hrow">
			{#each [0, 1] as i (i)}<Skeleton width={280} aspect={ratio} class="home-skel-rounded" />{/each}
		</div>
	{:else}
		<div class="home-hrow">
			{#each Array.from({ length: 5 }) as _item, i (i)}
				<Skeleton width={120} aspect="2 / 3" class="home-skel-rounded" />
			{/each}
		</div>
	{/if}
{/snippet}

{#snippet realBody(c: HomeComponent)}
	{@const v = c.value}
	{@const imgS = getImageScroller(v)}
	{@const bigS = getBigScroller(v)}
	{@const scr = getScroller(v)}
	{@const ml = getMangaList(v)}
	{@const mcl = getMangaChapterList(v)}
	{@const filters = getFilters(v)}
	{@const links = getLinks(v)}
	{#if imgS}
		<HomeImageScroller value={imgS} onOpenLink={openLink} />
	{:else if bigS}
		<div class="home-hrow">
			{#each bigS.entries as m (m.key)}
				<button
					type="button"
					class="home-big-card"
					disabled={pendingKey === m.key}
					onclick={() => void openManga(m)}
				>
					<span class="home-big-card__cover">
						<Cover src={proxyImage(m.cover)} title={m.title} />
						{#if m.content_rating === 'NSFW' || m.content_rating === 'Suggestive'}
							<span
								class={[
									'home-big-card__rating',
									m.content_rating === 'NSFW' && 'home-big-card__rating--nsfw'
								]}
							>
								{m.content_rating}
							</span>
						{/if}
					</span>
					<span class="home-big-card__main">
						<span class="home-big-card__title">{m.title}</span>
						{#if m.authors && m.authors.length > 0}
							<span class="home-big-card__author">{m.authors.join(', ')}</span>
						{/if}
						{#if m.description}<span class="home-big-card__desc">{m.description}</span>{/if}
						{#if m.tags && m.tags.length > 0}
							<span class="home-big-card__tags">{m.tags.join(' · ')}</span>
						{/if}
					</span>
				</button>
			{/each}
		</div>
	{:else if scr}
		<div class="home-hrow">
			{#each scr.entries as link, i (link.title + i)}
				<div class="home-scroller__item">
					<CoverCard
						src={proxyImage(link.image_url)}
						title={link.title}
						subtitle={link.subtitle ?? undefined}
						busy={linkBusy(link)}
						onclick={link.value ? () => openLink(link) : undefined}
					/>
				</div>
			{/each}
		</div>
	{:else if ml}
		{@const size = ml.page_size ?? 0}
		{#if size > 0}
			<!-- page_size 是横向分页语义：每页 size 条竖排成一列，列间横滑翻页 -->
			<div class="home-hrow home-pages">
				{#each chunk(ml.entries, size) as pageItems, pi (pi)}
					<div class="home-rows home-page">
						{#each pageItems as link, i (link.title + i)}
							{@render mangaRow(link, pi * size + i, ml.ranking)}
						{/each}
					</div>
				{/each}
			</div>
		{:else}
			<div class="home-rows">
				{#each ml.entries as link, i (link.title + i)}
					{@render mangaRow(link, i, ml.ranking)}
				{/each}
			</div>
		{/if}
	{:else if mcl}
		{@const size = mcl.page_size ?? 0}
		{#if size > 0}
			<div class="home-hrow home-pages">
				{#each chunk(mcl.entries, size) as pageItems, pi (pi)}
					<div class="home-rows home-page">
						{#each pageItems as mwc (mwc.manga.key + mwc.chapter.key)}
							{@render chapterRow(mwc)}
						{/each}
					</div>
				{/each}
			</div>
		{:else}
			<div class="home-rows">
				{#each mcl.entries as mwc (mwc.manga.key + mwc.chapter.key)}
					{@render chapterRow(mwc)}
				{/each}
			</div>
		{/if}
	{:else if filters}
		<div class="home-chips">
			{#each filters as item, i (item.title + i)}
				<Chip onclick={() => openFilterItem(item)}>{item.title}</Chip>
			{/each}
		</div>
	{:else if links}
		<div class="home-rows">
			{#each links as link, i (link.title + i)}
				<button
					type="button"
					class="home-row home-row--link"
					disabled={linkBusy(link)}
					onclick={() => openLink(link)}
				>
					<span class="home-row__main">
						<span class="home-row__title">{link.title}</span>
						{#if link.subtitle}<span class="home-row__subtitle">{link.subtitle}</span>{/if}
					</span>
					<ChevronRight size={16} class="home-row__chevron" />
				</button>
			{/each}
		</div>
	{/if}
{/snippet}

{#snippet section(c: HomeComponent)}
	{@const listing = sectionListing(c)}
	<section class="home-section">
		{#if c.title || c.subtitle || listing}
			<header class="home-section__head">
				<div class="home-section__titles">
					{#if c.title}<h2 class="home-section__title">{c.title}</h2>{/if}
					{#if c.subtitle}<p class="home-section__subtitle">{c.subtitle}</p>{/if}
				</div>
				{#if listing}
					<Button size="sm" onclick={() => openListing(listing)}>{$_('home.viewAll')}</Button>
				{/if}
			</header>
		{/if}
		{#if streaming && isPendingComponent(c)}
			{@render pendingBody(c)}
		{:else}
			{@render realBody(c)}
		{/if}
	</section>
{/snippet}

{#if source.data && !source.data.enabled}
	<div class="page container">
		<div class="source-page__head">
			<IconButton label={$_('home.backToBrowse')} onclick={() => goto('/browse')}>
				<ArrowLeft size={18} />
			</IconButton>
			<h1 class="source-page__title">{source.data.name}</h1>
		</div>
		<EmptyState
			icon={TriangleAlert}
			title={$_('home.sourceDisabledTitle')}
			hint={$_('home.sourceDisabledHint')}
		/>
	</div>
{:else}
	<div class="page container">
		<div class="source-page__head">
			<IconButton
				label={$_('home.backTo', {
					values: { name: source.data?.name ?? $_('home.sourceFallback') }
				})}
				onclick={() => goto(`/browse/${sourceId}`)}
			>
				<ArrowLeft size={18} />
			</IconButton>
			<h1 class="source-page__title">{source.data?.name ?? $_('home.sourceFallback')}</h1>
			<Button class="source-page__actions" onclick={() => goto(`/browse/${sourceId}`)}>
				<Search size={15} />
				{$_('common.search')}
			</Button>
		</div>

		{#if showSkeleton}
			{#each [0, 1] as i (i)}
				<div class="home-section">
					<Skeleton height={20} width={140} class="home-skel-title" />
					<div class="home-hrow">
						{#each Array.from({ length: 5 }) as _item, j (j)}
							<Skeleton width={120} aspect="2 / 3" class="home-skel-rounded" />
						{/each}
					</div>
				</div>
			{/each}
		{:else if homeQuery.isError}
			<EmptyState icon={TriangleAlert} title={$_('home.loadFailedTitle')} hint={homeQuery.error?.message}>
				{#snippet action()}
					<Button variant="primary" onclick={() => void homeQuery.refetch()}>
						{$_('common.retry')}
					</Button>
				{/snippet}
			</EmptyState>
		{:else if home == null}
			<EmptyState icon={House} title={$_('home.noHomeTitle')} hint={$_('home.noHomeHint')}>
				{#snippet action()}
					<Button variant="primary" onclick={() => goto(`/browse/${sourceId}`)}>
						{$_('home.goSearch')}
					</Button>
				{/snippet}
			</EmptyState>
		{:else if home.components.length === 0}
			<EmptyState icon={House} title={$_('home.emptyTitle')} hint={$_('home.emptyHint')} />
		{:else}
			{#each home.components as c, i (i)}
				{@render section(c)}
			{/each}
		{/if}
	</div>
{/if}

<style>
	/* Skeleton 子组件渲染独立 DOM，页面作用域选择器够不到；用 :global 并叠 .skeleton 提权压过基类圆角 */
	:global(.skeleton.home-skel-title) {
		margin-bottom: 10px;
	}
	:global(.skeleton.home-skel-rounded) {
		border-radius: var(--r-md);
	}
	:global(.skeleton.home-skel-chip) {
		border-radius: 999px;
	}
</style>
