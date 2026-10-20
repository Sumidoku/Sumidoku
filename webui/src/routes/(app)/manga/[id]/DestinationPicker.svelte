<script lang="ts">
	import { _ } from 'svelte-i18n';
	import GripVertical from '@lucide/svelte/icons/grip-vertical';
	import ListPlus from '@lucide/svelte/icons/list-plus';
	import Plus from '@lucide/svelte/icons/plus';
	import X from '@lucide/svelte/icons/x';
	import { sourceIconUrl } from '$lib/api/client';
	import { useSources } from '$lib/api/hooks';
	import type { Source } from '$lib/api/types';
	import { compareSourceName } from '$lib/source';
	import { Button } from '$lib/components';

	interface Props {
		/** 已选目标源 id，顺序即优先级 */
		selected: number[];
		onChange: (ids: number[]) => void;
		/** 源条目所在的源，目标组合要排除 */
		excludeId?: number;
	}

	let { selected, onChange, excludeId }: Props = $props();

	const sources = useSources();
	let dragIndex = $state<number | null>(null);
	let overIndex = $state<number | null>(null);
	// 图标加载失败的源 id 集合（对应 React 每行 RowIcon 的 failed 状态）
	let iconFailed = $state<Set<number>>(new Set());

	const all = $derived((sources.data ?? []).filter((s) => s.id !== excludeId));
	const byId = $derived(new Map(all.map((s) => [s.id, s])));
	const chosen = $derived(
		selected.map((sid) => byId.get(sid)).filter((s): s is Source => s !== undefined)
	);
	const chosenIds = $derived(new Set(chosen.map((s) => s.id)));
	// 可选源按名字字母序；置顶仍按 pinOrder 段在前
	const available = $derived(all.filter((s) => !chosenIds.has(s.id)).sort(compareSourceName));
	const pinnedRest = $derived(
		all.filter((s) => s.pinned && !chosenIds.has(s.id)).sort((a, b) => (a.pinOrder ?? 0) - (b.pinOrder ?? 0))
	);

	function markFailed(sid: number) {
		iconFailed = new Set(iconFailed).add(sid);
	}

	function addAllPinned() {
		onChange([...selected, ...pinnedRest.map((s) => s.id)]);
	}

	function onDrop(target: number) {
		const from = dragIndex;
		dragIndex = null;
		overIndex = null;
		if (from === null || from === target) return;
		const next = [...selected];
		const [moved] = next.splice(from, 1);
		next.splice(target, 0, moved);
		onChange(next);
	}
</script>

<div class="dest-picker">
	<div class="dest-picker__head">
		<h4>{$_('migrate.destSelectedHead')}</h4>
		{#if pinnedRest.length > 0}
			<Button size="sm" onclick={addAllPinned} title={$_('migrate.destAddAllPinnedTitle')}>
				<ListPlus size={14} />
				{$_('migrate.destAddAllPinned', { values: { n: pinnedRest.length } })}
			</Button>
		{/if}
	</div>
	{#if chosen.length === 0}
		<p class="muted">{$_('migrate.destEmpty')}</p>
	{:else}
		<div class="dest-picker__list">
			{#each chosen as s, i (s.id)}
				<div
					class={[
						'dest-picker__row',
						dragIndex === i && 'dest-picker__row--dragging',
						overIndex === i && dragIndex !== null && dragIndex !== i && 'dest-picker__row--over'
					]}
					role="listitem"
					ondragover={(e) => {
						e.preventDefault();
						overIndex = i;
					}}
					ondrop={() => onDrop(i)}
				>
					<span
						class="dest-picker__grip"
						role="button"
						tabindex="-1"
						aria-label="drag"
						draggable="true"
						ondragstart={(e) => {
							e.dataTransfer?.setData('text/plain', s.key);
							if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
							dragIndex = i;
						}}
						ondragend={() => {
							dragIndex = null;
							overIndex = null;
						}}
					>
						<GripVertical size={15} />
					</span>
					<span class="dest-picker__order">{i + 1}</span>
					{#if iconFailed.has(s.id)}
						<span class="dest-picker__icon dest-picker__icon--fallback">{s.name.slice(0, 1)}</span>
					{:else}
						<span class="dest-picker__icon">
							<img src={sourceIconUrl(s.id)} alt="" loading="lazy" onerror={() => markFailed(s.id)} />
						</span>
					{/if}
					<span class="dest-picker__name">{s.name}</span>
					<button
						type="button"
						class="dest-picker__remove"
						title={$_('migrate.destRemove', { values: { name: s.name } })}
						onclick={() => onChange(selected.filter((sid) => sid !== s.id))}
					>
						<X size={15} />
					</button>
				</div>
			{/each}
		</div>
	{/if}

	<h4>{$_('migrate.destAvailable')}</h4>
	{#if available.length === 0}
		<p class="muted">{$_('migrate.destNoneAvailable')}</p>
	{:else}
		<div class="dest-picker__list">
			{#each available as s (s.id)}
				<button
					type="button"
					class="dest-picker__row dest-picker__row--add"
					title={$_('migrate.destAdd', { values: { name: s.name } })}
					onclick={() => onChange([...selected, s.id])}
				>
					{#if iconFailed.has(s.id)}
						<span class="dest-picker__icon dest-picker__icon--fallback">{s.name.slice(0, 1)}</span>
					{:else}
						<span class="dest-picker__icon">
							<img src={sourceIconUrl(s.id)} alt="" loading="lazy" onerror={() => markFailed(s.id)} />
						</span>
					{/if}
					<span class="dest-picker__name">{s.name}</span>
					{#if s.pinned}<span class="dest-picker__pin">{$_('migrate.destPin')}</span>{/if}
					<Plus size={15} />
				</button>
			{/each}
		</div>
	{/if}
</div>
