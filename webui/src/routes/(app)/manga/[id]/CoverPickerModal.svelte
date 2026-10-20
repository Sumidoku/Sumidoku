<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Check from '@lucide/svelte/icons/check';
	import Images from '@lucide/svelte/icons/images';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { ApiError, sourceImageUrl } from '$lib/api/client';
	import { useAlternateCovers, useSelectCover } from '$lib/api/hooks';
	import type { MangaDetail } from '$lib/api/types';
	import { Button, EmptyState, Modal, Spinner, pushToast } from '$lib/components';

	let { manga, onClose }: { manga: MangaDetail; onClose: () => void } = $props();

	const coversQuery = useAlternateCovers(() => manga.id);
	const selectCover = useSelectCover();
	let pendingUrl = $state<string | null>(null);

	function pick(url: string) {
		if (selectCover.isPending || url === manga.cover) return;
		pendingUrl = url;
		selectCover.mutate(
			{ id: manga.id, url },
			{
				onSuccess: () => {
					pushToast($_('manga.coverChanged'), 'info');
					onClose();
				},
				onSettled: () => (pendingUrl = null)
			}
		);
	}

	// 源没导出 get_alternate_covers 时服务端按约定 501，按「没有备选」显示而不是报错
	const unsupported = $derived(coversQuery.error instanceof ApiError && coversQuery.error.status === 501);
	const covers = $derived(coversQuery.data ?? []);
</script>

<Modal open title={$_('manga.changeCover')} {onClose} panelClassName="cover-picker">
	{#if coversQuery.isPending}
		<div class="cover-picker__loading">
			<Spinner size={20} />
		</div>
	{:else if unsupported}
		<EmptyState icon={Images} title={$_('manga.coverUnsupportedTitle')} />
	{:else if coversQuery.isError}
		<EmptyState
			icon={TriangleAlert}
			title={$_('manga.coverLoadFailTitle')}
			hint={coversQuery.error?.message}
		>
			{#snippet action()}
				<Button variant="primary" onclick={() => void coversQuery.refetch()}>
					{$_('common.retry')}
				</Button>
			{/snippet}
		</EmptyState>
	{:else if covers.length === 0}
		<EmptyState icon={Images} title={$_('manga.coverEmptyTitle')} />
	{:else}
		<div class="cover-picker__grid">
			{#each covers as url (url)}
				{@const current = url === manga.cover}
				<button
					type="button"
					class={['cover-picker__cell', current && 'cover-picker__cell--current']}
					disabled={selectCover.isPending}
					title={current ? $_('manga.coverCurrent') : $_('manga.coverSet')}
					onclick={() => pick(url)}
				>
					<!-- 防盗链源的图直连必 403，统一走服务端代理 -->
					<img src={sourceImageUrl(manga.sourceId!, url)} alt="" loading="lazy" />
					{#if current}
						<span class="cover-picker__badge">
							<Check size={13} />
							{$_('manga.coverCurrentBadge')}
						</span>
					{/if}
					{#if pendingUrl === url}
						<span class="cover-picker__busy">
							<Spinner size={16} />
						</span>
					{/if}
				</button>
			{/each}
		</div>
	{/if}
</Modal>
