<script lang="ts">
	import { goto } from '$app/navigation';
	import { marked } from 'marked';
	import { _ } from 'svelte-i18n';
	import BookOpen from '@lucide/svelte/icons/book-open';
	import ArrowDownUp from '@lucide/svelte/icons/arrow-down-up';
	import ArrowLeftRight from '@lucide/svelte/icons/arrow-left-right';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import EyeOff from '@lucide/svelte/icons/eye-off';
	import Images from '@lucide/svelte/icons/images';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import Search from '@lucide/svelte/icons/search';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { coverUrl } from '$lib/api/client';
	import {
		queryClient,
		qk,
		useAddToLibrary,
		useBatchChapters,
		useCategories,
		useEnqueueDownloads,
		useManga,
		useMangaChapters,
		usePatchManga,
		useRefreshChapters,
		useRefreshManga,
		useRemoveFromLibrary,
		useSetMangaCategories,
		useSource,
		useUpdateChapter
	} from '$lib/api/hooks';
	import type { Chapter } from '$lib/api/types';
	import { statusText } from '$lib/format';
	import { applyPrefs, loadPrefs, nextTri, savePrefs } from '$lib/chapterPrefs';
	import type { ChapterPrefs, SortKey, Tri } from '$lib/chapterPrefs';
	import {
		Button,
		Chip,
		Cover,
		EmptyState,
		IconButton,
		Input,
		Modal,
		pushToast,
		Select,
		Skeleton,
		Spinner,
		Switch
	} from '$lib/components';
	import ChapterRow from './ChapterRow.svelte';
	import type { ChapterDlState } from './ChapterRow.svelte';
	import CoverPickerModal from './CoverPickerModal.svelte';
	import MigrateModal from './MigrateModal.svelte';
	import MoreMenu from './MoreMenu.svelte';

	type ChipColor = 'default' | 'accent' | 'danger' | 'success' | 'warning';

	let { id }: { id: number } = $props();

	const STATUS_COLOR: Record<string, ChipColor> = {
		ongoing: 'accent',
		completed: 'success',
		hiatus: 'warning',
		cancelled: 'danger',
		unknown: 'default'
	};

	type ChapFilterKey = 'unread' | 'downloaded' | 'bookmarked' | 'locked';

	const CHAP_FILTERS: { value: ChapFilterKey; labelKey: string }[] = [
		{ value: 'unread', labelKey: 'manga.filterUnread' },
		{ value: 'downloaded', labelKey: 'manga.filterDownloaded' },
		{ value: 'bookmarked', labelKey: 'manga.filterBookmarked' },
		{ value: 'locked', labelKey: 'manga.filterLocked' }
	];

	const SORT_OPTIONS: { value: SortKey; labelKey: string }[] = [
		{ value: 'source', labelKey: 'manga.sortSource' },
		{ value: 'chapter', labelKey: 'manga.sortChapter' },
		{ value: 'date', labelKey: 'manga.sortDate' }
	];

	/** 三态 chip 颜色：含=accent、排除=danger、关=default */
	function triColor(t: Tri): ChipColor {
		return t === 1 ? 'accent' : t === -1 ? 'danger' : 'default';
	}

	function triTitleKey(t: Tri): string {
		return t === 1 ? 'manga.triInclude' : t === -1 ? 'manga.triExclude' : 'manga.triOff';
	}

	const mangaQuery = useManga(() => id);
	const chaptersQuery = useMangaChapters(() => id);
	const manga = $derived(mangaQuery.data ?? null);
	const chapters = $derived(chaptersQuery.data ?? []);
	// 源 config 的 tag/artist/author 搜索能力位；manga 未到时以 0 占位查不到，自然不渲染链接
	const sourceQuery = useSource(() => manga?.sourceId ?? 0);

	let descOpen = $state(false);
	let removeOpen = $state(false);
	let catOpen = $state(false);
	let catSel = $state<Set<number>>(new Set());

	let search = $state('');
	let migrateOpen = $state(false);
	let coverPickerOpen = $state(false);
	// 排序/筛选设置按漫画持久化；本组件按 id keyed，换漫画整组件重挂，天然隔离
	let prefs = $state<ChapterPrefs>(loadPrefs(id));
	let selecting = $state(false);
	let selected = $state<Set<number>>(new Set());

	// 入队只记本地即时反馈，下载进度在下载页看
	let dlQueued = $state<Set<number>>(new Set());
	let dlPending = $state<Set<number>>(new Set());

	// prefs 变化即回写该部漫画的存储
	$effect(() => {
		if (Number.isFinite(id)) savePrefs(id, prefs);
	});

	const addToLibrary = useAddToLibrary();
	const removeFromLibrary = useRemoveFromLibrary();
	const setMangaCategories = useSetMangaCategories();
	const patchManga = usePatchManga();
	const refreshManga = useRefreshManga();
	const refreshChapters = useRefreshChapters();
	const updateChapter = useUpdateChapter();
	const batchChapters = useBatchChapters();
	const enqueueDownloads = useEnqueueDownloads();
	const categoriesQuery = useCategories();

	const unread = $derived(chapters.filter((c) => !c.isRead).length);
	const continueCh = $derived(
		chapters.length === 0 ? null : (chapters.find((c) => !c.isRead) ?? chapters[chapters.length - 1])
	);

	// 语言/扫图组只用于渲染筛选项，从全部章节里去重取值
	const languages = $derived.by(() => {
		const set = new Set<string>();
		for (const c of chapters) if (c.language) set.add(c.language);
		return [...set];
	});

	const scanlators = $derived.by(() => {
		const set = new Set<string>();
		for (const c of chapters) for (const s of c.scanlators ?? []) set.add(s);
		return [...set];
	});

	const visible = $derived.by(() => {
		// 先应用持久化的排序/筛选，再做标题/序号文本搜索
		let list = applyPrefs(chapters, prefs);
		const q = search.trim().toLowerCase();
		if (q) {
			list = list.filter(
				(c) =>
					(c.title ?? '').toLowerCase().includes(q) ||
					(c.chapter_number !== null && String(c.chapter_number).includes(q))
			);
		}
		return list;
	});

	const allSelected = $derived(visible.length > 0 && visible.every((c) => selected.has(c.id)));
	const batchBusy = $derived(batchChapters.isPending || enqueueDownloads.isPending);

	// wire 上 authors/artists/tags 可能是 null（aidoku Option<Vec>），不是空数组
	const authors = $derived(manga?.authors ?? []);
	const artists = $derived(manga?.artists ?? []);
	const tags = $derived(manga?.tags ?? []);
	const sourceConfig = $derived(sourceQuery.data?.config);
	// 去重保序；同时在 authors/artists 出现的名字按 author 搜索（Aidoku 搜作者同款）
	const creators = $derived(
		[...new Set([...authors, ...artists])].map((name) => ({
			name,
			filterId: authors.includes(name) ? ('author' as const) : ('artist' as const)
		}))
	);
	const statusColor = $derived(STATUS_COLOR[(manga?.status ?? '').toLowerCase()] ?? 'default');
	const allCats = $derived(categoriesQuery.data ?? []);
	const descHtml = $derived(manga?.description ? (marked.parse(manga.description) as string) : '');

	function readTarget(c: Chapter): string {
		// 已读章节重读从第 1 页开始，未读有进度才续
		const resume = !c.isRead && c.lastPageRead > 0 ? c.lastPageRead : 0;
		return `/reader/${c.id}?manga=${id}&page=${resume}`;
	}

	function activate(c: Chapter) {
		// 章节刷新流中的占位行（负 id）没有库内 id，点读/选择都等终态落地
		if (c.id < 0) return;
		if (selecting) {
			const next = new Set(selected);
			if (next.has(c.id)) next.delete(c.id);
			else next.add(c.id);
			selected = next;
		} else {
			goto(readTarget(c));
		}
	}

	function exitSelect() {
		selecting = false;
		selected = new Set();
	}

	function toggleAll() {
		selected = allSelected ? new Set() : new Set(visible.filter((c) => c.id > 0).map((c) => c.id));
	}

	function openCats() {
		catSel = new Set(manga?.categoryIds ?? []);
		catOpen = true;
	}

	function toggleCat(cid: number) {
		const next = new Set(catSel);
		if (next.has(cid)) next.delete(cid);
		else next.add(cid);
		catSel = next;
	}

	function saveCats() {
		setMangaCategories.mutate(
			{ mangaId: id, categoryIds: [...catSel] },
			{
				onSuccess: () => {
					pushToast($_('manga.catSaved'), 'info');
					catOpen = false;
				}
			}
		);
	}

	function confirmRemove() {
		removeFromLibrary.mutate(id, {
			onSuccess: () => {
				pushToast($_('manga.removedFromLibrary'), 'info');
				removeOpen = false;
			}
		});
	}

	function dlStateOf(c: Chapter): ChapterDlState {
		if (dlPending.has(c.id)) return 'pending';
		if (dlQueued.has(c.id)) return 'queued';
		return 'idle';
	}

	function downloadChapter(c: Chapter) {
		if (c.id < 0 || dlPending.has(c.id) || dlQueued.has(c.id) || c.isDownloaded) return;
		dlPending = new Set(dlPending).add(c.id);
		enqueueDownloads.mutate([c.id], {
			onSuccess: () => {
				dlQueued = new Set(dlQueued).add(c.id);
				pushToast($_('manga.queuedOne'), 'info');
			},
			onSettled: () => {
				const next = new Set(dlPending);
				next.delete(c.id);
				dlPending = next;
			}
		});
	}

	function batch(body: { isRead?: boolean; isBookmarked?: boolean }, toastKey: string) {
		batchChapters.mutate(
			{ chapterIds: [...selected], ...body },
			{
				onSuccess: (res) => {
					pushToast($_(toastKey, { values: { n: res.updated } }), 'info');
					void queryClient.invalidateQueries({ queryKey: qk.chapters(id) });
					void queryClient.invalidateQueries({ queryKey: qk.library });
					exitSelect();
				}
			}
		);
	}

	function downloadSelected() {
		const ids = [...selected];
		enqueueDownloads.mutate(ids, {
			onSuccess: (res) => {
				pushToast($_('manga.queuedN', { values: { n: res.queued } }), 'info');
				dlQueued = new Set([...dlQueued, ...ids]);
				exitSelect();
			}
		});
	}

	// tag/artist/author 搜索：Aidoku 以 query 空 + Text 过滤器发起，复用浏览页 filters 深链
	function searchByTextFilter(filterId: string, value: string) {
		if (!manga?.sourceId) return;
		const filters = encodeURIComponent(JSON.stringify([{ Text: { id: filterId, value } }]));
		goto(`/browse/${manga.sourceId}?filters=${filters}`);
	}
</script>

{#if mangaQuery.isPending}
	<div class="page container">
		<div class="manga-skeleton">
			<Skeleton width={180} aspect="2 / 3" class="skeleton--cover" />
			<div class="manga-skeleton__info">
				<Skeleton height={24} width="60%" />
				<Skeleton height={14} width="40%" />
				<Skeleton height={14} width="85%" />
				<Skeleton height={14} width="70%" />
			</div>
		</div>
		<div class="manga-skeleton-rows">
			{#each Array.from({ length: 10 }) as _row, i (i)}
				<Skeleton height={44} />
			{/each}
		</div>
	</div>
{:else if mangaQuery.isError || !manga}
	<div class="page container">
		<EmptyState
			icon={TriangleAlert}
			title={$_('manga.loadFailTitle')}
			hint={mangaQuery.error?.message || $_('manga.loadFailHint')}
		>
			{#snippet action()}
				<Button
					variant="primary"
					onclick={() => {
						void mangaQuery.refetch();
						void chaptersQuery.refetch();
					}}
				>
					{$_('common.retry')}
				</Button>
			{/snippet}
		</EmptyState>
	</div>
{:else}
	<div class="page container">
		<div class="manga-hero">
			<div
				class="manga-hero__bg"
				style={`background-image: url("${coverUrl(id, manga.cover)}")`}
			></div>
			<div class="manga-hero__scrim"></div>
			<div class="manga-hero__body">
				<div class="manga-hero__cover">
					{#if manga.content_rating === 'NSFW'}
						<span class="manga-hero__nsfw">NSFW</span>
					{/if}
					<Cover src={coverUrl(id, manga.cover)} title={manga.title} />
					<button type="button" class="manga-hero__cover-edit" onclick={() => (coverPickerOpen = true)}>
						<Images size={13} />
						{$_('manga.changeCover')}
					</button>
				</div>
				<div class="manga-hero__info">
					<h1 class="manga-hero__title">{manga.title}</h1>
					{#if creators.length > 0}
						<div class="manga-hero__creators">
							{#each creators as c, i (c.name)}
								{@const searchable =
									c.filterId === 'author'
										? sourceConfig?.supportsAuthorSearch
										: sourceConfig?.supportsArtistSearch}
								<span>
									{#if i > 0}{' / '}{/if}
									{#if searchable}
										<button
											type="button"
											class="manga-hero__creator"
											title={$_(c.filterId === 'author' ? 'manga.searchAuthor' : 'manga.searchArtist', {
												values: { name: c.name }
											})}
											onclick={() => searchByTextFilter(c.filterId, c.name)}
										>
											{c.name}
										</button>
									{:else}
										{c.name}
									{/if}
								</span>
							{/each}
						</div>
					{/if}
					<div class="manga-hero__chips">
						<Chip color={statusColor}>{statusText(manga.status)}</Chip>
						{#if manga.url}
							<a
								class="manga-hero__source"
								href={manga.url}
								target="_blank"
								rel="noreferrer"
								title={$_('manga.sourceTitle')}
							>
								<ExternalLink size={13} />
								{$_('manga.source')}
							</a>
						{/if}
					</div>
					{#if tags.length > 0}
						<div class="manga-hero__tags">
							{#each tags as tg (tg)}
								<Chip
									title={sourceConfig?.supportsTagSearch
										? $_('manga.searchTag', { values: { tag: tg } })
										: undefined}
									onclick={sourceConfig?.supportsTagSearch
										? () => searchByTextFilter('tag', tg)
										: undefined}
								>
									{tg}
								</Chip>
							{/each}
						</div>
					{/if}
					{#if manga.description}
						<div class={['manga-hero__desc', !descOpen && 'manga-hero__desc--clamp']}>
							{@html descHtml}
						</div>
						<button
							type="button"
							class="manga-hero__desc-toggle"
							onclick={() => (descOpen = !descOpen)}
						>
							{descOpen ? $_('manga.collapse') : $_('manga.expand')}
						</button>
					{/if}
					<div class="manga-hero__actions">
						<Button
							variant="primary"
							disabled={!continueCh}
							onclick={() => continueCh && goto(readTarget(continueCh))}
						>
							<BookOpen size={16} />
							{$_('manga.continueReading')}
						</Button>
						{#if manga.inLibrary}
							<Button variant="danger" onclick={() => (removeOpen = true)}>
								{$_('manga.removeFromLibrary')}
							</Button>
							<Button onclick={openCats}>{$_('manga.categories')}</Button>
						{:else}
							<Button
								variant="primary"
								loading={addToLibrary.isPending}
								onclick={() =>
									addToLibrary.mutate(id, {
										onSuccess: () => pushToast($_('manga.addedToLibrary'), 'info')
									})}
							>
								{$_('manga.addToLibrary')}
							</Button>
						{/if}
						<IconButton
							label={$_('manga.refreshDetail')}
							disabled={refreshManga.isPending}
							onclick={() => refreshManga.mutate(id)}
						>
							{#if refreshManga.isPending}<Spinner size={16} />{:else}<RefreshCw size={17} />{/if}
						</IconButton>
						<IconButton
							label={manga.incognito ? $_('manga.incognitoOn') : $_('manga.incognitoOff')}
							class={manga.incognito ? 'icon-btn--active' : undefined}
							disabled={patchManga.isPending}
							onclick={() => patchManga.mutate({ id, incognito: !manga.incognito })}
						>
							<EyeOff size={17} />
						</IconButton>
						<MoreMenu>
							{#snippet children(close)}
								<label class="manga-menu__row">
									<span>{$_('manga.autoUpdate')}</span>
									<Switch
										checked={manga.update_strategy === 'Always'}
										disabled={patchManga.isPending}
										onChange={(on) =>
											patchManga.mutate({ id, updateStrategy: on ? 'Always' : 'Never' })}
										label={$_('manga.autoUpdate')}
									/>
								</label>
								<div class="manga-menu__hint">{$_('manga.autoUpdateHint')}</div>
								<button
									type="button"
									class="manga-menu__row"
									disabled={refreshChapters.isPending}
									onclick={() =>
										refreshChapters.mutate(id, {
											onSuccess: () => {
												pushToast($_('manga.chaptersRefreshed'), 'info');
												close();
											}
										})}
								>
									<span>{$_('manga.refreshChapters')}</span>
									{#if refreshChapters.isPending}<Spinner size={14} />{:else}<RefreshCw size={14} />{/if}
								</button>
								{#if manga.inLibrary}
									<button
										type="button"
										class="manga-menu__row"
										onclick={() => {
											close();
											migrateOpen = true;
										}}
									>
										<span>{$_('manga.switchSource')}</span>
										<ArrowLeftRight size={14} />
									</button>
								{/if}
							{/snippet}
						</MoreMenu>
					</div>
				</div>
			</div>
		</div>

		<div class="manga-chaps">
			<div class="manga-chaps__head">
				<h2>{$_('manga.chaptersTitle')}</h2>
				<span class="faint">
					{$_('manga.chaptersSummary', { values: { total: chapters.length, unread } })}
				</span>
			</div>

			{#if selecting}
				<div class="manga-chaps__select-tools">
					<Button size="sm" onclick={toggleAll}>
						{allSelected ? $_('manga.deselectAll') : $_('manga.selectAll')}
					</Button>
					<span class="manga-chaps__selected">
						{$_('manga.selectedCount', { values: { n: selected.size } })}
					</span>
					<Button size="sm" onclick={exitSelect}>{$_('manga.done')}</Button>
				</div>
			{:else}
				<div class="manga-chaps__tools">
					<Input type="search" placeholder={$_('manga.searchChapters')} bind:value={search}>
						{#snippet icon()}<Search size={15} />{/snippet}
					</Input>
					<Select
						value={prefs.sort}
						title={$_('manga.sortLabel')}
						aria-label={$_('manga.sortLabel')}
						onchange={(e) => (prefs = { ...prefs, sort: e.currentTarget.value as SortKey })}
					>
						{#each SORT_OPTIONS as o (o.value)}
							<option value={o.value}>{$_(o.labelKey)}</option>
						{/each}
					</Select>
					<Button
						size="sm"
						title={prefs.asc ? $_('manga.ascTitle') : $_('manga.descTitle')}
						onclick={() => (prefs = { ...prefs, asc: !prefs.asc })}
					>
						<ArrowDownUp size={14} />
						{prefs.asc ? $_('manga.asc') : $_('manga.desc')}
					</Button>
					<div class="manga-chaps__filters">
						{#each CHAP_FILTERS as f (f.value)}
							<Chip
								color={triColor(prefs.filters[f.value])}
								class={prefs.filters[f.value] === -1 ? 'chip--exclude' : undefined}
								title={$_(triTitleKey(prefs.filters[f.value]), { values: { label: $_(f.labelKey) } })}
								onclick={() =>
									(prefs = {
										...prefs,
										filters: { ...prefs.filters, [f.value]: nextTri(prefs.filters[f.value]) }
									})}
							>
								{$_(f.labelKey)}
							</Chip>
						{/each}
					</div>
					{#if languages.length > 1}
						<Select
							value={prefs.language ?? ''}
							title={$_('manga.languageLabel')}
							aria-label={$_('manga.languageLabel')}
							onchange={(e) => {
								const v = e.currentTarget.value;
								prefs = { ...prefs, language: v || null };
							}}
						>
							<option value="">{$_('manga.allLanguages')}</option>
							{#each languages as l (l)}
								<option value={l}>{l}</option>
							{/each}
						</Select>
					{/if}
					{#if scanlators.length > 1}
						<Select
							value={prefs.scanlators[0] ?? ''}
							title={$_('manga.scanlatorLabel')}
							aria-label={$_('manga.scanlatorLabel')}
							onchange={(e) => {
								const v = e.currentTarget.value;
								prefs = { ...prefs, scanlators: v ? [v] : [] };
							}}
						>
							<option value="">{$_('manga.allScanlators')}</option>
							{#each scanlators as s (s)}
								<option value={s}>{s}</option>
							{/each}
						</Select>
					{/if}
					{#if chapters.length > 0}
						<Button size="sm" onclick={() => (selecting = true)}>{$_('manga.select')}</Button>
					{/if}
				</div>
			{/if}

			{#if chaptersQuery.isPending}
				<div class="manga-skeleton-rows">
					{#each Array.from({ length: 10 }) as _row, i (i)}
						<Skeleton height={44} />
					{/each}
				</div>
			{:else if chaptersQuery.isError}
				<EmptyState
					icon={TriangleAlert}
					title={$_('manga.chaptersLoadFailTitle')}
					hint={chaptersQuery.error?.message}
				>
					{#snippet action()}
						<Button variant="primary" onclick={() => void chaptersQuery.refetch()}>
							{$_('common.retry')}
						</Button>
					{/snippet}
				</EmptyState>
			{:else if chapters.length === 0}
				<EmptyState icon={BookOpen} title={$_('manga.noChaptersTitle')} hint={$_('manga.noChaptersHint')} />
			{:else if visible.length === 0}
				<EmptyState icon={Search} title={$_('manga.noMatchTitle')} hint={$_('manga.noMatchHint')} />
			{:else}
				<div class={selecting ? 'manga-chaps__list--selecting' : undefined}>
					{#each visible as c (c.id)}
						<ChapterRow
							chapter={c}
							{selecting}
							selected={selected.has(c.id)}
							onActivate={() => activate(c)}
							dlState={dlStateOf(c)}
							onDownload={() => downloadChapter(c)}
							onToggleBookmark={() => {
								if (c.id < 0) return;
								updateChapter.mutate({ id: c.id, mangaId: id, isBookmarked: !c.isBookmarked });
							}}
							bookmarkPending={updateChapter.isPending && updateChapter.variables?.id === c.id}
						/>
					{/each}
				</div>
			{/if}
		</div>

		{#if selecting}
			<div class="manga-batch">
				<span class="manga-batch__count">
					{$_('manga.selectedBar', { values: { n: selected.size } })}
				</span>
				<Button
					size="sm"
					disabled={selected.size === 0 || batchBusy}
					onclick={() => batch({ isRead: true }, 'manga.markedReadN')}
				>
					{$_('manga.markRead')}
				</Button>
				<Button
					size="sm"
					disabled={selected.size === 0 || batchBusy}
					onclick={() => batch({ isRead: false }, 'manga.markedUnreadN')}
				>
					{$_('manga.markUnread')}
				</Button>
				<Button
					size="sm"
					disabled={selected.size === 0 || batchBusy}
					onclick={() => batch({ isBookmarked: true }, 'manga.bookmarkedN')}
				>
					{$_('manga.addBookmark')}
				</Button>
				<Button
					size="sm"
					disabled={selected.size === 0 || batchBusy}
					onclick={() => batch({ isBookmarked: false }, 'manga.unbookmarkedN')}
				>
					{$_('manga.removeBookmark')}
				</Button>
				<Button
					size="sm"
					variant="primary"
					disabled={selected.size === 0 || batchBusy}
					onclick={downloadSelected}
				>
					{$_('manga.download')}
				</Button>
			</div>
		{/if}

		{#if migrateOpen}
			<MigrateModal {manga} onClose={() => (migrateOpen = false)} />
		{/if}

		{#if coverPickerOpen}
			<CoverPickerModal {manga} onClose={() => (coverPickerOpen = false)} />
		{/if}

		<Modal open={removeOpen} title={$_('manga.removeFromLibrary')} onClose={() => (removeOpen = false)}>
			<p>{$_('manga.removeQuestion', { values: { title: manga.title } })}</p>
			{#snippet footer()}
				<Button onclick={() => (removeOpen = false)}>{$_('common.cancel')}</Button>
				<Button variant="danger" loading={removeFromLibrary.isPending} onclick={confirmRemove}>
					{$_('manga.removeConfirm')}
				</Button>
			{/snippet}
		</Modal>

		<Modal open={catOpen} title={$_('manga.catTitle')} onClose={() => (catOpen = false)}>
			{#if allCats.length === 0}
				<p class="muted">{$_('manga.catEmpty')}</p>
			{:else}
				<div class="manga-cats">
					{#each allCats as c (c.id)}
						<label class="manga-cats__item">
							<span>{c.name}</span>
							<Switch checked={catSel.has(c.id)} onChange={() => toggleCat(c.id)} label={c.name} />
						</label>
					{/each}
				</div>
			{/if}
			{#snippet footer()}
				{#if allCats.length > 0}
					<Button variant="primary" loading={setMangaCategories.isPending} onclick={saveCats}>
						{$_('common.save')}
					</Button>
				{/if}
			{/snippet}
		</Modal>
	</div>
{/if}
