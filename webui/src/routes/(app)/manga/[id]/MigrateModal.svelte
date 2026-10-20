<script lang="ts">
	import { goto } from '$app/navigation';
	import { _ } from 'svelte-i18n';
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import Search from '@lucide/svelte/icons/search';
	import { coverUrl } from '$lib/api/client';
	import { migrateLookup, useMigrate, useRegisterManga, useSources } from '$lib/api/hooks';
	import type { Manga, MangaDetail, MigrateMode } from '$lib/api/types';
	import { mapPool } from '$lib/pool';
	import { Button, Cover, Modal, Spinner, pushToast } from '$lib/components';
	import DestinationPicker from './DestinationPicker.svelte';

	let { manga, onClose }: { manga: MangaDetail; onClose: () => void } = $props();

	const MODES: { mode: MigrateMode; labelKey: string; hintKey: string }[] = [
		{ mode: 'copy', labelKey: 'migrate.modeCopy', hintKey: 'migrate.modeCopyHint' },
		{ mode: 'migrate', labelKey: 'migrate.modeMigrate', hintKey: 'migrate.modeMigrateHint' },
		{
			mode: 'migrateWithHistory',
			labelKey: 'migrate.modeMigrateHistory',
			hintKey: 'migrate.modeMigrateHistoryHint'
		}
	];

	let destinations = $state<number[]>([]);
	let searching = $state(false);
	let results = $state<Manga[] | null>(null);
	let picked = $state<Manga | null>(null);
	let error = $state<string | null>(null);

	const migrate = useMigrate();
	const register = useRegisterManga();
	const sources = useSources();

	const sourceName = (sid: number) => (sources.data ?? []).find((s) => s.id === sid)?.name ?? '';

	async function search() {
		searching = true;
		results = null;
		picked = null;
		error = null;
		const hits = await mapPool(destinations, 3, async (destId) => {
			try {
				return await migrateLookup(manga.id, destId);
			} catch (e) {
				error = e instanceof Error ? e.message : $_('migrate.searchFail');
				return null;
			}
		});
		results = hits.flatMap((h) => h?.candidates ?? []);
		searching = false;
	}

	function execute(mode: MigrateMode, labelKey: string) {
		if (!picked) return;
		migrate.mutate(
			{
				mode,
				entries: [
					{
						fromMangaId: manga.id,
						toSourceId: picked.sourceId!,
						toMangaKey: picked.key,
						toManga: picked
					}
				]
			},
			{
				onSuccess: (res) => {
					const item = res.results[0];
					if (!item?.ok) {
						error = item?.message || item?.reason || $_('migrate.migrateFail');
						return;
					}
					for (const w of item.warnings ?? []) pushToast(w, 'info');
					pushToast($_('migrate.done', { values: { mode: $_(labelKey) } }), 'info');
					onClose();
					goto(`/manga/${item.toMangaId}`);
				}
			}
		);
	}

	function showEntry() {
		if (!picked) return;
		register.mutate(
			{ sourceId: picked.sourceId!, manga: picked },
			{
				onSuccess: (res) => {
					onClose();
					goto(`/manga/${res.id}`);
				},
				onError: (e) => (error = e.message)
			}
		);
	}

	const busy = $derived(migrate.isPending || register.isPending);
</script>

<Modal
	open
	title={$_('migrate.switchSourceTitle', { values: { title: manga.title } })}
	{onClose}
	panelClassName="migrate-single"
>
	{#if picked}
		<div class="migrate-single__confirm">
			<div class="migrate-single__pair">
				<span class="migrate-row__cover">
					<Cover src={coverUrl(manga.id, manga.cover)} title={manga.title} />
				</span>
				<ArrowRight size={16} />
				<div>
					<div class="migrate-row__title">{picked.title}</div>
					<div class="migrate-row__meta">{sourceName(picked.sourceId!)}</div>
				</div>
			</div>
			{#if error}<p class="migrate-single__error">{error}</p>{/if}
			<div class="migrate-single__actions">
				{#each MODES as m (m.mode)}
					<Button size="sm" disabled={busy} title={$_(m.hintKey)} onclick={() => execute(m.mode, m.labelKey)}>
						{$_(m.labelKey)}
					</Button>
				{/each}
				<Button size="sm" disabled={busy} onclick={showEntry}>{$_('migrate.onlyViewEntry')}</Button>
				<Button size="sm" disabled={busy} onclick={() => (picked = null)}>
					{$_('migrate.backToCandidates')}
				</Button>
			</div>
			<p class="muted">{$_('migrate.modeMigrateHint')}</p>
		</div>
	{:else}
		<DestinationPicker
			selected={destinations}
			onChange={(ids) => (destinations = ids)}
			excludeId={manga.sourceId}
		/>
		<div class="migrate-single__bar">
			<Button
				size="sm"
				variant="primary"
				loading={searching}
				disabled={destinations.length === 0}
				onclick={() => void search()}
			>
				<Search size={14} />
				{$_('migrate.searchCandidates')}
			</Button>
			{#if error}<span class="migrate-single__error">{error}</span>{/if}
		</div>
		{#if searching}
			<Spinner />
		{:else if results === null}
			<p class="muted">{$_('migrate.concurrentHint')}</p>
		{:else if results.length === 0}
			<p class="muted">{$_('migrate.noHits')}</p>
		{:else}
			<div class="migrate-rematch__list">
				{#each results as c (`${c.sourceId}:${c.key}`)}
					<button type="button" class="migrate-rematch__item" onclick={() => (picked = c)}>
						<span class="migrate-rematch__title">{c.title}</span>
						<span class="migrate-rematch__source">{sourceName(c.sourceId!)}</span>
					</button>
				{/each}
			</div>
		{/if}
	{/if}
</Modal>
