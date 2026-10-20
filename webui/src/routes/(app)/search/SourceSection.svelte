<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { sourceImageUrl } from '$lib/api/client';
	import type { Manga, Source, SourceSearchResult } from '$lib/api/types';
	import { statusText } from '$lib/format';
	import { Button, CoverCard, Skeleton } from '$lib/components';

	interface Props {
		source: Source;
		result?: SourceSearchResult;
		/** busy 态用 `<sourceId>:<key>` 复合键：不同源可能撞同 key */
		pendingKey: string | null;
		onOpen: (m: Manga) => void;
		onViewAll: () => void;
	}
	let { source, result, pendingKey, onOpen, onViewAll }: Props = $props();

	const busyKey = (m: Manga) => `${m.sourceId}:${m.key}`;
</script>

<section class="home-section">
	<header class="home-section__head">
		<div class="home-section__titles">
			<h2 class="home-section__title">{source.name}</h2>
		</div>
		<Button size="sm" onclick={onViewAll}>{$_('search.viewAll')}</Button>
	</header>
	{#if !result}
		<div class="home-hrow">
			{#each Array.from({ length: 5 }) as _item, i (i)}
				<Skeleton width={120} aspect="2 / 3" class="search-skeleton" />
			{/each}
		</div>
	{:else if result.error}
		<p class="search-page__error">{result.error.message}</p>
	{:else}
		<div class="home-hrow">
			{#each result.entries as m (m.key)}
				<div class="home-scroller__item">
					<CoverCard
						src={m.cover ? sourceImageUrl(source.id, m.cover) : m.cover}
						title={m.title}
						subtitle={m.authors?.[0] ?? statusText(m.status)}
						busy={pendingKey === busyKey(m)}
						onclick={() => onOpen(m)}
					/>
				</div>
			{/each}
		</div>
	{/if}
</section>

<style>
	/* React 版骨架 style={{ borderRadius: 'var(--r-md)' }}；Skeleton 默认是 --r-sm */
	:global(.search-skeleton) {
		border-radius: var(--r-md);
	}
</style>
