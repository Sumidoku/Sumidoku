<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import ArrowLeftRight from '@lucide/svelte/icons/arrow-left-right';
	import CircleArrowUp from '@lucide/svelte/icons/circle-arrow-up';
	import FolderPlus from '@lucide/svelte/icons/folder-plus';
	import PackageOpen from '@lucide/svelte/icons/package-open';
	import ScanSearch from '@lucide/svelte/icons/scan-search';
	import type { Source } from '$lib/api/types';
	import {
		useDeleteSource,
		usePinSource,
		useReorderPinned,
		useScanSources,
		useSources,
		useUpdateAllSources
	} from '$lib/api/hooks';
	import { compareSourceName } from '$lib/source';
	import { Button, EmptyState, Modal, PageHeader, SegmentedControl, Skeleton, pushToast } from '$lib/components';
	import InternalSourceModal from './InternalSourceModal.svelte';
	import ReposPanel from './ReposPanel.svelte';
	import SourceCard from './SourceCard.svelte';

	const sources = useSources();
	const scan = useScanSources();
	const del = useDeleteSource();
	const updateAll = useUpdateAllSources();
	const pin = usePinSource();
	const reorderPinned = useReorderPinned();

	let pendingDelete = $state<Source | null>(null);
	let registeringInternal = $state(false);
	let tab = $state<'installed' | 'repos'>('installed');
	let dragIndex = $state<number | null>(null);
	let overIndex = $state<number | null>(null);

	// 已装源按名字字母序；置顶段另按 pinOrder 排
	const all = $derived([...(sources.data ?? [])].sort(compareSourceName));
	// Aidoku 同款独立分段：可更新的源聚在一起，置顶段之下、全部列表之上
	const updateSources = $derived(all.filter((s) => s.updateVersion !== null));
	const updateCount = $derived(updateSources.length);
	const pinnedSources = $derived(
		all.filter((s) => s.pinned).sort((a, b) => (a.pinOrder ?? 0) - (b.pinOrder ?? 0))
	);

	async function onScan() {
		try {
			const r = await scan.mutateAsync();
			pushToast(
				$_('browse.scanDone', {
					values: { installed: r.installed.length, updated: r.updated.length, skipped: r.skipped.length }
				}),
				'info'
			);
			for (const s of r.skipped.slice(0, 3)) {
				pushToast($_('browse.scanSkipped', { values: { dir: s.dir, reason: s.reason } }), 'info');
			}
		} catch {
			// client 已弹错误 toast
		}
	}

	async function onUpdateAll() {
		try {
			const r = await updateAll.mutateAsync();
			pushToast(
				$_('browse.updateAllDone', { values: { updated: r.updated.length, failed: r.failed.length } }),
				'info'
			);
			for (const f of r.failed.slice(0, 3)) {
				pushToast($_('browse.updateFailed', { values: { key: f.key, reason: f.reason } }), 'info');
			}
			for (const w of r.migrationWarnings.slice(0, 3)) {
				pushToast($_('browse.migrationWarning', { values: { key: w.key, message: w.message } }), 'info');
			}
			if (r.migrationRequired.length > 0) {
				pushToast($_('browse.migrationNeededMulti', { values: { keys: r.migrationRequired.join('、') } }), 'info');
				goto('/migrate');
			}
		} catch {
			// client 已弹错误 toast
		}
	}

	async function togglePin(s: Source) {
		try {
			await pin.mutateAsync({ id: s.id, pinned: !s.pinned });
			pushToast(
				s.pinned
					? $_('browse.unpinnedToast', { values: { name: s.name } })
					: $_('browse.pinnedToast', { values: { name: s.name } }),
				'info'
			);
		} catch {
			// client 已弹错误 toast
		}
	}

	function onDropPinned(target: number) {
		const from = dragIndex;
		dragIndex = null;
		overIndex = null;
		if (from === null || from === target) return;
		const keys = pinnedSources.map((s) => s.key);
		const [moved] = keys.splice(from, 1);
		keys.splice(target, 0, moved);
		reorderPinned.mutate(keys);
	}

	async function confirmDelete() {
		if (!pendingDelete) return;
		try {
			await del.mutateAsync(pendingDelete.id);
			pushToast($_('browse.uninstalledToast', { values: { name: pendingDelete.name } }), 'info');
			pendingDelete = null;
		} catch {
			// client 已弹错误 toast
		}
	}
</script>

{#snippet headerActions()}
	<Button onclick={() => goto('/migrate')}>
		<ArrowLeftRight size={15} />
		{$_('browse.migrate')}
	</Button>
	{#if updateCount > 0}
		<Button loading={updateAll.isPending} onclick={() => void onUpdateAll()}>
			<CircleArrowUp size={15} />
			{$_('browse.updateAll', { values: { n: updateCount } })}
		</Button>
	{/if}
	<Button onclick={() => (registeringInternal = true)}>
		<FolderPlus size={15} />
		{$_('browse.addInternal')}
	</Button>
	<Button variant="primary" loading={scan.isPending} onclick={() => void onScan()}>
		<ScanSearch size={15} />
		{$_('browse.scan')}
	</Button>
{/snippet}

<div class="page container">
	<PageHeader title={$_('nav.browse')} actions={tab === 'installed' ? headerActions : undefined} />

	<div class="browse__tabs">
		<SegmentedControl
			options={[
				{ value: 'installed', label: $_('browse.tabInstalled') },
				{ value: 'repos', label: $_('browse.tabRepos') }
			]}
			value={tab}
			onChange={(v) => (tab = v as 'installed' | 'repos')}
		/>
	</div>

	{#if tab === 'repos'}
		<ReposPanel />
	{:else if sources.isPending}
		<div class="source-list">
			{#each Array.from({ length: 4 }) as _item, i (i)}
				<Skeleton height={82} />
			{/each}
		</div>
	{:else if all.length === 0}
		<EmptyState icon={PackageOpen} title={$_('browse.emptyTitle')} hint={$_('browse.emptyHint')} />
	{:else}
		{#if pinnedSources.length > 0}
			<h2 class="browse__section">{$_('browse.sectionPinned')}</h2>
			<div class="source-list">
				{#each pinnedSources as s, i (s.id)}
					<SourceCard
						source={s}
						dnd={{
							dragging: dragIndex === i,
							dragOver: overIndex === i && dragIndex !== null && dragIndex !== i,
							onDragStart: (e) => {
								e.dataTransfer?.setData('text/plain', s.key);
								if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
								dragIndex = i;
							},
							onDragOver: (e) => {
								e.preventDefault();
								overIndex = i;
							},
							onDrop: () => onDropPinned(i),
							onDragEnd: () => {
								dragIndex = null;
								overIndex = null;
							}
						}}
						onTogglePin={(s) => void togglePin(s)}
						onDelete={(s) => (pendingDelete = s)}
					/>
				{/each}
			</div>
		{/if}
		{#if updateSources.length > 0}
			<h2 class="browse__section">{$_('browse.sectionUpdates', { values: { n: updateSources.length } })}</h2>
			<div class="source-list">
				{#each updateSources as s (s.id)}
					<SourceCard
						source={s}
						onTogglePin={(s) => void togglePin(s)}
						onDelete={(s) => (pendingDelete = s)}
					/>
				{/each}
			</div>
		{/if}
		{#if pinnedSources.length > 0 || updateSources.length > 0}
			<!-- 上方出现任何分段时，全列表需要「全部」标题区隔 -->
			<h2 class="browse__section">{$_('browse.sectionAll')}</h2>
		{/if}
		<div class="source-list">
			{#each all as s (s.id)}
				<SourceCard
					source={s}
					onTogglePin={(s) => void togglePin(s)}
					onDelete={(s) => (pendingDelete = s)}
				/>
			{/each}
		</div>
	{/if}

	<Modal
		open={pendingDelete !== null}
		title={$_('browse.uninstallTitle', { values: { name: pendingDelete?.name ?? '' } })}
		onClose={() => (pendingDelete = null)}
	>
		<p class="muted">{$_('browse.uninstallBody')}</p>
		{#snippet footer()}
			<Button onclick={() => (pendingDelete = null)}>{$_('common.cancel')}</Button>
			<Button variant="danger" loading={del.isPending} onclick={() => void confirmDelete()}>
				{$_('browse.uninstall')}
			</Button>
		{/snippet}
	</Modal>

	<InternalSourceModal open={registeringInternal} onClose={() => (registeringInternal = false)} />
</div>
