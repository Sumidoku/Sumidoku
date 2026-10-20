<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import Clock from '@lucide/svelte/icons/clock';
	import { coverUrl } from '$lib/api/client';
	import { useHistory } from '$lib/api/hooks';
	import { chapterLabel, relativeTime } from '$lib/format';
	import { Button, Cover, EmptyState, InfiniteList, PageHeader, Skeleton } from '$lib/components';

	const query = useHistory();
	const entries = $derived(query.data?.pages.flatMap((p) => p.entries) ?? []);
</script>

<div class="page container">
	<PageHeader title={$_('nav.history')} />

	{#if query.isPending}
		<div class="history-list">
			{#each Array.from({ length: 6 }) as _item, i (i)}
				<Skeleton height={76} />
			{/each}
		</div>
	{:else if entries.length === 0}
		<EmptyState icon={Clock} title={$_('history.emptyTitle')} hint={$_('history.emptyHint')}>
			{#snippet action()}
				<Button variant="primary" onclick={() => goto('/library')}>
					{$_('history.toLibrary')}
				</Button>
			{/snippet}
		</EmptyState>
	{:else}
		<InfiniteList
			hasNextPage={query.hasNextPage}
			isFetching={query.isFetching}
			onLoadMore={() => void query.fetchNextPage()}
		>
			<div class="history-list">
				{#each entries as h (h.chapter.id)}
					<a
						class="history-item card"
						href={`/reader/${h.chapter.id}?manga=${h.manga.id}&page=${h.lastPageRead}`}
					>
						<div class="history-item__thumb">
							<Cover src={coverUrl(h.manga.id, h.manga.cover)} title={h.manga.title} />
						</div>
						<div class="history-item__meta">
							<div class="history-item__title">{h.manga.title}</div>
							<div class="muted history-item__chapter">{chapterLabel(h.chapter)}</div>
						</div>
						<div class="history-item__right">
							<span class="faint">{relativeTime(h.lastReadAt)}</span>
							{#if h.isRead}
								<span class="faint">{$_('history.readDone')}</span>
							{:else}
								<span class="history-item__progress">
									{$_('history.readProgress', { values: { n: h.lastPageRead + 1 } })}
								</span>
							{/if}
						</div>
					</a>
				{/each}
			</div>
		</InfiniteList>
	{/if}
</div>

<style>
	.history-list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.history-item {
		display: flex;
		align-items: center;
		gap: 12px;
		transition: background 0.15s;
	}

	.history-item:hover {
		background: var(--bg-hover);
	}

	.history-item__thumb {
		width: 56px;
		flex: none;
	}

	.history-item__thumb :global(.cover),
	.history-item__thumb :global(.cover__placeholder) {
		border-radius: var(--r-sm);
	}

	.history-item__meta {
		flex: 1;
		min-width: 0;
	}

	.history-item__title {
		font-weight: 600;
		font-size: 14.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.history-item__chapter {
		font-size: 13.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.history-item__right {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 2px;
		font-size: 12.5px;
		flex: none;
	}

	.history-item__progress {
		color: var(--accent);
	}
</style>
