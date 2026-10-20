<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { page } from '$app/state';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import Search from '@lucide/svelte/icons/search';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { coverUrl } from '$lib/api/client';
	import { migrateLookup, useLibrary, useMigrate, useMigrateSources } from '$lib/api/hooks';
	import type { LibraryEntry, Manga, MigrateMode, MigrateResultItem } from '$lib/api/types';
	import { mapPool } from '$lib/pool';
	import { compareSourceName } from '$lib/source';
	import { Button, Cover, EmptyState, Input, Modal, PageHeader, Skeleton, Spinner, pushToast } from '$lib/components';
	import DestinationPicker from './DestinationPicker.svelte';

	type Step = 'source' | 'series' | 'destinations' | 'match';

	interface Row {
		manga: LibraryEntry;
		state: 'pending' | 'matching' | 'matched' | 'notfound';
		target?: Manga;
		result?: MigrateResultItem;
	}

	const MODES: MigrateMode[] = ['copy', 'migrate', 'migrateWithHistory'];
	const MODE_LABEL_KEY: Record<MigrateMode, string> = {
		copy: 'migrate.modeCopy',
		migrate: 'migrate.modeMigrate',
		migrateWithHistory: 'migrate.modeMigrateWithHistory'
	};
	const MODE_HINT_KEY: Record<MigrateMode, string> = {
		copy: 'migrate.modeCopyHint',
		migrate: 'migrate.modeMigrateHint',
		migrateWithHistory: 'migrate.modeMigrateWithHistoryHint'
	};

	// breaking-change 引导入口：?source=<id> 预填来源，sameSource=1 允许并预选同源目标
	const params = page.url.searchParams;
	const prefill = Number(params.get('source')) || null;
	const sameSource = params.get('sameSource') === '1';

	let step = $state<Step>(prefill ? 'series' : 'source');
	let sourceId = $state<number | null>(prefill);
	let selected = $state<Set<number>>(new Set());
	let destinations = $state<number[]>(sameSource && prefill ? [prefill] : []);
	let rows = $state<Row[]>([]);
	let matching = $state(false);
	let confirmMode = $state<MigrateMode | null>(null);
	let executed = $state(false);

	// 单行手动重搜状态
	let rematchRow = $state<Row | null>(null);
	let rematchQuery = $state('');
	let rematchSearching = $state(false);
	let rematchResults = $state<Manga[] | null>(null);

	// 行内「更多操作」菜单：同时只开一个
	let openMenuId = $state<number | null>(null);

	const sourcesQuery = useMigrateSources();
	const libraryQuery = useLibrary(
		() => undefined,
		() => ({ sourceId: sourceId ?? 0 })
	);
	const migrate = useMigrate();

	// 迁移源清单按名字字母序，与已装源列表一致
	const migrateSources = $derived([...(sourcesQuery.data ?? [])].sort(compareSourceName));
	const series = $derived(
		sourceId === null ? [] : (libraryQuery.data?.pages.flatMap((p) => p.entries) ?? [])
	);
	const sourceName = $derived((sourcesQuery.data ?? []).find((s) => s.id === sourceId)?.name ?? '');
	const destName = (id: number) =>
		(sourcesQuery.data ?? []).find((s) => s.id === id)?.name ?? `#${id}`;

	const matchedRows = $derived(rows.filter((r) => r.state === 'matched' && r.target));
	const doneCount = $derived(rows.filter((r) => r.result?.ok).length);

	function patchRow(id: number, patch: Partial<Row>) {
		rows = rows.map((r) => (r.manga.id === id ? { ...r, ...patch } : r));
	}

	/** 批量自动匹配：按目标优先级逐源标题搜索，首个有结果的源胜出并取其第一条 */
	async function autoMatch(
		list: Row[],
		dests: number[],
		update: (id: number, patch: Partial<Row>) => void
	) {
		await mapPool(list, 3, async (row) => {
			update(row.manga.id, { state: 'matching' });
			for (const destId of dests) {
				try {
					const r = await migrateLookup(row.manga.id, destId);
					if (r.candidates.length > 0) {
						update(row.manga.id, { state: 'matched', target: r.candidates[0] });
						return;
					}
				} catch {
					// 单个目标源搜索失败按没命中处理，继续下一个目标源
				}
			}
			update(row.manga.id, { state: 'notfound' });
		});
	}

	function toggle(id: number) {
		const next = new Set(selected);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		selected = next;
	}

	function pickSource(id: number) {
		sourceId = id;
		selected = new Set();
		step = 'series';
	}

	function startMatch() {
		const picked = series.filter((m) => selected.has(m.id));
		rows = picked.map((manga) => ({ manga, state: 'pending' }));
		executed = false;
		step = 'match';
		matching = true;
		const initial: Row[] = picked.map((manga) => ({ manga, state: 'pending' }));
		void autoMatch(initial, destinations, patchRow).finally(() => (matching = false));
	}

	function reasonText(item: MigrateResultItem): string {
		switch (item.reason ?? '') {
			case 'not_found':
				return $_('migrate.reasonNotFound');
			case 'not_in_library':
				return $_('migrate.reasonNotInLibrary');
			case 'source_not_found':
				return $_('migrate.reasonSourceNotFound');
			case 'same_source':
				return $_('migrate.reasonSameSource');
			case 'fetch_failed':
				return item.message
					? $_('migrate.reasonFetchFailedMessage', { values: { message: item.message } })
					: $_('migrate.reasonFetchFailed');
			default:
				return item.reason ?? '';
		}
	}

	function execute(mode: MigrateMode) {
		const entries = matchedRows
			.filter((r) => !r.result?.ok)
			.map((r) => ({
				fromMangaId: r.manga.id,
				toSourceId: r.target!.sourceId!,
				toMangaKey: r.target!.key,
				toManga: r.target
			}));
		if (entries.length === 0) return;
		migrate.mutate(
			{ mode, entries },
			{
				onSuccess: (res) => {
					executed = true;
					confirmMode = null;
					for (const item of res.results) {
						patchRow(item.fromMangaId, { result: item });
					}
					const failed = res.results.filter((r) => !r.ok).length;
					pushToast(
						failed === 0
							? $_('migrate.toastAllDone', { values: { n: res.results.length } })
							: $_('migrate.toastPartial', {
									values: { n: res.results.length - failed, m: failed }
								}),
						'info'
					);
					for (const item of res.results) {
						for (const w of item.warnings ?? []) pushToast(w, 'info');
					}
				}
			}
		);
	}

	function openRematch(row: Row) {
		rematchRow = row;
		rematchQuery = row.manga.title;
		rematchSearching = false;
		rematchResults = null;
	}

	/** 单行手动重搜：对全部目标源 3 并发标题搜索，用户挑候选 */
	async function rematchSearch() {
		const row = rematchRow;
		if (!row) return;
		rematchSearching = true;
		rematchResults = null;
		const hits = await mapPool(destinations, 3, async (destId) => {
			try {
				return await migrateLookup(row.manga.id, destId, rematchQuery);
			} catch {
				return null;
			}
		});
		rematchResults = hits.flatMap((h) => h?.candidates ?? []);
		rematchSearching = false;
	}

	function pickRematch(candidate: Manga) {
		if (rematchRow) patchRow(rematchRow.manga.id, { state: 'matched', target: candidate });
		rematchRow = null;
	}

	function back() {
		step = step === 'series' ? 'source' : step === 'destinations' ? 'series' : 'destinations';
	}
</script>

<div class="page container">
	<PageHeader title={$_('migrate.title')}>
		{#snippet actions()}
			{#if step !== 'source'}
				<Button size="sm" onclick={back}>
					<ArrowLeft size={14} />
					{$_('migrate.prevStep')}
				</Button>
			{/if}
		{/snippet}
	</PageHeader>

	{#if step === 'source'}
		{#if sourcesQuery.isPending}
			<div class="migrate-source-list">
				{#each Array.from({ length: 3 }) as _item, i (i)}
					<Skeleton height={64} class="migrate-source-skel" />
				{/each}
			</div>
		{:else}
			<p class="muted">{$_('migrate.pickSourcePrompt')}</p>
			<div class="migrate-source-list">
				{#each migrateSources.filter((s) => s.mangaCount > 0) as s (s.id)}
					<button type="button" class="migrate-source" onclick={() => pickSource(s.id)}>
						<span class="migrate-source__name">{s.name}</span>
						<span class="migrate-source__count"
							>{$_('migrate.inLibraryCount', { values: { n: s.mangaCount } })}</span
						>
						<ArrowRight size={16} />
					</button>
				{/each}
			</div>
			{#if migrateSources.every((s) => s.mangaCount === 0)}
				<EmptyState
					icon={TriangleAlert}
					title={$_('migrate.emptyLibraryTitle')}
					hint={$_('migrate.emptyLibraryHint')}
				/>
			{/if}
		{/if}
	{/if}

	{#if step === 'series'}
		<div class="migrate-toolbar">
			<span>
				{$_('migrate.sourceLabel')}<b>{sourceName}</b> · {$_('migrate.selectedCount', {
					values: { n: selected.size, total: series.length }
				})}
			</span>
			<Button
				size="sm"
				onclick={() =>
					(selected = selected.size === series.length ? new Set() : new Set(series.map((m) => m.id)))}
			>
				{selected.size === series.length ? $_('migrate.clearAll') : $_('migrate.selectAll')}
			</Button>
			<Button
				size="sm"
				variant="primary"
				disabled={selected.size === 0}
				onclick={() => (step = 'destinations')}
			>
				{$_('migrate.pickDestinations')}
				<ArrowRight size={14} />
			</Button>
		</div>
		<div class="migrate-series">
			{#each series as m (m.id)}
				<label class="migrate-series__item">
					<input type="checkbox" checked={selected.has(m.id)} onchange={() => toggle(m.id)} />
					<span class="migrate-series__cover">
						<Cover src={coverUrl(m.id, m.cover)} title={m.title} />
					</span>
					<span class="migrate-series__title">{m.title}</span>
				</label>
			{/each}
		</div>
		{#if libraryQuery.hasNextPage}
			<Button
				size="sm"
				loading={libraryQuery.isFetchingNextPage}
				onclick={() => void libraryQuery.fetchNextPage()}
			>
				{$_('migrate.loadMore')}
			</Button>
		{/if}
	{/if}

	{#if step === 'destinations'}
		<DestinationPicker
			selected={destinations}
			onChange={(ids) => (destinations = ids)}
			excludeId={sameSource ? undefined : (sourceId ?? undefined)}
		/>
		<div class="migrate-toolbar">
			<Button variant="primary" disabled={destinations.length === 0} onclick={startMatch}>
				{$_('migrate.startMatch', { values: { n: selected.size } })}
				<ArrowRight size={14} />
			</Button>
		</div>
	{/if}

	{#if step === 'match'}
		<div class="migrate-toolbar">
			<span>
				{$_('migrate.destLabel')}{destinations.map(destName).join(' → ')} · {$_(
					'migrate.matchedCount',
					{ values: { n: matchedRows.length, total: rows.length } }
				)}{#if matching}
					· {$_('migrate.matching')}{/if}{#if executed}
					· {$_('migrate.migratedCount', { values: { n: doneCount } })}{/if}
			</span>
			{#if matching}<Spinner size={16} />{/if}
			{#if !matching && !executed}
				<Button
					size="sm"
					onclick={() =>
						void autoMatch(
							rows.filter((r) => r.state === 'notfound'),
							destinations,
							patchRow
						)}
				>
					{$_('migrate.retryNotFound')}
				</Button>
				{#each MODES as mode (mode)}
					<Button
						size="sm"
						variant={mode === 'migrate' ? 'primary' : undefined}
						disabled={matchedRows.length === 0}
						onclick={() => (confirmMode = mode)}
					>
						{$_(MODE_LABEL_KEY[mode])}
					</Button>
				{/each}
			{/if}
		</div>
		<div class="migrate-rows">
			{#each rows as row (row.manga.id)}
				<div class="migrate-row">
					<span class="migrate-row__cover">
						<Cover src={coverUrl(row.manga.id, row.manga.cover)} title={row.manga.title} />
					</span>
					<div class="migrate-row__side">
						<div class="migrate-row__title">{row.manga.title}</div>
						<div class="migrate-row__meta">{sourceName}</div>
					</div>
					<span class="migrate-row__arrow"><ArrowRight size={16} /></span>
					<div class="migrate-row__side">
						{#if row.state === 'matching' || (matching && row.state === 'pending')}
							<Spinner size={16} />
						{:else if row.target}
							<div class="migrate-row__title">{row.target.title}</div>
							<div class="migrate-row__meta">{destName(row.target.sourceId!)}</div>
						{:else}
							<div class="migrate-row__notfound">{$_('migrate.notFound')}</div>
						{/if}
						{#if row.result}
							{#if row.result.ok}
								<a class="migrate-row__goto" href={`/manga/${row.result.toMangaId}`}>
									{$_('migrate.goToTarget')}
								</a>
							{:else}
								<span class="migrate-row__reason">{reasonText(row.result)}</span>
							{/if}
						{/if}
					</div>
					{#if !executed && !matching}
						<span class="migrate-row__menu">
							<button
								type="button"
								class="migrate-row__menu-btn"
								title={$_('migrate.moreActions')}
								onclick={() => (openMenuId = openMenuId === row.manga.id ? null : row.manga.id)}
							>
								<Ellipsis size={16} />
							</button>
							{#if openMenuId === row.manga.id}
								<span class="migrate-row__menu-pop card">
									<button
										type="button"
										onclick={() => {
											openMenuId = null;
											openRematch(row);
										}}
									>
										{$_('migrate.rematch')}
									</button>
									<button
										type="button"
										onclick={() => {
											openMenuId = null;
											rows = rows.filter((r) => r.manga.id !== row.manga.id);
										}}
									>
										{$_('migrate.skipThis')}
									</button>
								</span>
							{/if}
						</span>
					{/if}
				</div>
			{/each}
		</div>
	{/if}

	<Modal
		open={confirmMode !== null}
		title={confirmMode
			? $_('migrate.confirmTitle', {
					values: { mode: $_(MODE_LABEL_KEY[confirmMode]), n: matchedRows.length }
				})
			: ''}
		onClose={() => (confirmMode = null)}
	>
		{#snippet footer()}
			<Button onclick={() => (confirmMode = null)}>{$_('common.cancel')}</Button>
			<Button
				variant="primary"
				loading={migrate.isPending}
				onclick={() => confirmMode && execute(confirmMode)}
			>
				{confirmMode ? $_('migrate.confirmButton', { values: { mode: $_(MODE_LABEL_KEY[confirmMode]) } }) : ''}
			</Button>
		{/snippet}
		<p class="muted">{confirmMode ? $_(MODE_HINT_KEY[confirmMode]) : ''}</p>
		<p class="muted">
			{$_('migrate.notMatchedNote', { values: { n: rows.length - matchedRows.length } })}
		</p>
	</Modal>

	<Modal
		open={rematchRow !== null}
		title={rematchRow
			? $_('migrate.rematchTitle', { values: { title: rematchRow.manga.title } })
			: ''}
		onClose={() => (rematchRow = null)}
		panelClassName="migrate-rematch"
	>
		<div class="migrate-rematch__bar">
			<Input
				bind:value={rematchQuery}
				onkeydown={(e) => e.key === 'Enter' && void rematchSearch()}
			>
				{#snippet icon()}<Search size={15} />{/snippet}
			</Input>
			<Button
				size="sm"
				variant="primary"
				loading={rematchSearching}
				disabled={!rematchQuery.trim()}
				onclick={() => void rematchSearch()}
			>
				{$_('common.search')}
			</Button>
		</div>
		{#if rematchSearching}
			<Spinner />
		{:else if rematchResults === null}
			<p class="muted">{$_('migrate.rematchPrompt')}</p>
		{:else if rematchResults.length === 0}
			<p class="muted">{$_('migrate.rematchEmpty')}</p>
		{:else}
			<div class="migrate-rematch__list">
				{#each rematchResults as c (`${c.sourceId}:${c.key}`)}
					<button type="button" class="migrate-rematch__item" onclick={() => pickRematch(c)}>
						<span class="migrate-rematch__title">{c.title}</span>
						<span class="migrate-rematch__source">{destName(c.sourceId!)}</span>
					</button>
				{/each}
			</div>
		{/if}
	</Modal>
</div>

<style>
	:global(.migrate-source-skel) {
		border-radius: var(--r-md);
	}

	.migrate-toolbar {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
		margin: 12px 0;
	}

	.migrate-source-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 8px;
	}

	.migrate-source {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px 14px;
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elev);
		color: var(--text);
		font: inherit;
		cursor: pointer;
		text-align: left;
	}

	.migrate-source:hover {
		background: var(--bg-hover);
	}

	.migrate-source__name {
		flex: 1;
		font-weight: 600;
	}

	.migrate-source__count {
		color: var(--text-dim);
		font-size: 13px;
	}

	.migrate-series {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: 10px;
		margin-bottom: 12px;
	}

	.migrate-series__item {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px;
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elev);
		cursor: pointer;
	}

	.migrate-series__cover {
		flex: none;
		width: 32px;
		aspect-ratio: 2 / 3;
		border-radius: 4px;
		overflow: hidden;
	}

	.migrate-series__title {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 13px;
	}

	.migrate-rows {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.migrate-row {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 8px 10px;
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elev);
	}

	.migrate-row__cover {
		flex: none;
		width: 36px;
		aspect-ratio: 2 / 3;
		border-radius: 4px;
		overflow: hidden;
	}

	.migrate-row__side {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.migrate-row__title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 14px;
	}

	.migrate-row__meta {
		font-size: 12px;
		color: var(--text-faint);
	}

	.migrate-row__arrow {
		flex: none;
		display: inline-flex;
		color: var(--text-faint);
	}

	.migrate-row__notfound {
		color: var(--warning);
		font-size: 13px;
	}

	.migrate-row__goto {
		font-size: 12px;
		color: var(--accent);
	}

	.migrate-row__reason {
		font-size: 12px;
		color: var(--danger);
	}

	.migrate-row__menu {
		position: relative;
		flex: none;
	}

	.migrate-row__menu-btn {
		display: inline-flex;
		border: none;
		background: none;
		color: var(--text-faint);
		cursor: pointer;
		padding: 4px;
	}

	.migrate-row__menu-pop {
		position: absolute;
		right: 0;
		top: calc(100% + 4px);
		z-index: 30;
		display: flex;
		flex-direction: column;
		min-width: 120px;
		padding: 4px;
	}

	.migrate-row__menu-pop button {
		border: none;
		background: none;
		color: var(--text);
		font: inherit;
		text-align: left;
		padding: 8px 10px;
		border-radius: var(--r-sm);
		cursor: pointer;
	}

	.migrate-row__menu-pop button:hover {
		background: var(--bg-hover);
	}

	/* 手动重搜 / 单条候选 */

	.migrate-rematch__bar {
		display: flex;
		gap: 8px;
		align-items: center;
		margin-bottom: 10px;
	}

	.migrate-rematch__bar :global(.input) {
		flex: 1;
	}

	.migrate-rematch__list {
		display: flex;
		flex-direction: column;
		gap: 6px;
		max-height: 50vh;
		overflow-y: auto;
	}

	.migrate-rematch__item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		padding: 8px 10px;
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elev);
		color: var(--text);
		font: inherit;
		cursor: pointer;
		text-align: left;
	}

	.migrate-rematch__item:hover {
		background: var(--bg-hover);
	}

	.migrate-rematch__title {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.migrate-rematch__source {
		flex: none;
		font-size: 12px;
		color: var(--text-faint);
	}

	/* 弹窗加宽：候选列表与选择器都偏挤 */
	:global(.migrate-rematch) {
		width: min(560px, 92vw);
	}
</style>
