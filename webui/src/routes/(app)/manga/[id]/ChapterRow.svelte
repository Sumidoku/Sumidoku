<script module lang="ts">
	/** idle 可点下载；pending 入队请求中；queued 已入队（下载进度去下载页看） */
	export type ChapterDlState = 'idle' | 'pending' | 'queued';
</script>

<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Bookmark from '@lucide/svelte/icons/bookmark';
	import Check from '@lucide/svelte/icons/check';
	import CircleCheckBig from '@lucide/svelte/icons/circle-check-big';
	import Download from '@lucide/svelte/icons/download';
	import FileDown from '@lucide/svelte/icons/file-down';
	import Lock from '@lucide/svelte/icons/lock';
	import { chapterFileUrl } from '$lib/api/client';
	import type { Chapter } from '$lib/api/types';
	import { chapterLabel, formatDate } from '$lib/format';
	import { IconButton, Spinner } from '$lib/components';

	interface Props {
		chapter: Chapter;
		selecting: boolean;
		selected: boolean;
		/** 普通模式进阅读，选择模式切选中 */
		onActivate: () => void;
		dlState: ChapterDlState;
		onDownload: () => void;
		onToggleBookmark: () => void;
		bookmarkPending: boolean;
	}

	let {
		chapter: c,
		selecting,
		selected,
		onActivate,
		dlState,
		onDownload,
		onToggleBookmark,
		bookmarkPending
	}: Props = $props();

	const uploaded = $derived(formatDate(c.date_uploaded));
	// wire 上 scanlators 可能是 null（aidoku Option<Vec>），不是空数组
	const scanlator = $derived(c.scanlators?.[0] ?? '');
</script>

<div
	class={['chapter-row', c.isRead && 'chapter-row--read', selecting && selected && 'chapter-row--selected']}
	role="button"
	tabindex="0"
	onclick={onActivate}
	onkeydown={(e) => {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			onActivate();
		}
	}}
>
	{#if selecting}
		<span class="chapter-row__check">
			{#if selected}<Check size={12} strokeWidth={3} />{/if}
		</span>
	{/if}
	<span class="chapter-row__num">{c.chapter_number ?? '—'}</span>
	<span class="chapter-row__main">
		<span class="chapter-row__title">{chapterLabel(c)}</span>
		{#if uploaded || scanlator}
			<span class="chapter-row__meta">
				{#if uploaded}<span>{uploaded}</span>{/if}
				{#if scanlator}<span>{scanlator}</span>{/if}
			</span>
		{/if}
	</span>
	<span class="chapter-row__status">
		{#if c.locked}
			<span class="chapter-row__lock" title={$_('manga.filterLocked')}>
				<Lock size={14} />
			</span>
		{/if}
		{#if c.isBookmarked}
			<span class="chapter-row__bm" title={$_('manga.filterBookmarked')}>
				<Bookmark size={14} fill="currentColor" />
			</span>
		{/if}
		{#if c.isDownloaded}
			<span class="chapter-row__dl-ok" title={$_('manga.filterDownloaded')}>
				<CircleCheckBig size={14} />
			</span>
		{/if}
		{#if !c.isRead && c.lastPageRead > 0}
			<span class="chapter-row__prog">
				{$_('manga.pageShort', { values: { n: c.lastPageRead + 1 } })}
			</span>
		{/if}
	</span>
	{#if !selecting}
		<span class="chapter-row__actions">
			{#if c.isDownloaded}
				<a
					class="chapter-row__cbz"
					href={chapterFileUrl(c.id)}
					download
					title={$_('manga.downloadCbz')}
					aria-label={$_('manga.downloadCbz')}
					onclick={(e) => e.stopPropagation()}
				>
					<FileDown size={16} />
				</a>
			{:else if dlState === 'idle'}
				<IconButton
					label={$_('manga.downloadChapter')}
					onclick={(e) => {
						e.stopPropagation();
						onDownload();
					}}
				>
					<Download size={16} />
				</IconButton>
			{:else}
				<IconButton
					label={dlState === 'pending' ? $_('manga.queuePending') : $_('manga.queuedState')}
					disabled
				>
					<Spinner size={15} />
				</IconButton>
			{/if}
			<IconButton
				label={c.isBookmarked ? $_('manga.removeBookmark') : $_('manga.addBookmark')}
				disabled={bookmarkPending}
				onclick={(e) => {
					e.stopPropagation();
					onToggleBookmark();
				}}
			>
				<Bookmark size={16} fill={c.isBookmarked ? 'currentColor' : 'none'} />
			</IconButton>
		</span>
	{/if}
</div>
