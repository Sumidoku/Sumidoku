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
	// 图标加载失败按源 id 记，退回首字母占位
	let failed = $state<Record<number, boolean>>({});

	const all = $derived((sources.data ?? []).filter((s) => s.id !== excludeId));
	const byId = $derived(new Map(all.map((s) => [s.id, s])));
	const chosen = $derived(
		selected.map((id) => byId.get(id)).filter((s): s is Source => s !== undefined)
	);
	const chosenIds = $derived(new Set(chosen.map((s) => s.id)));
	// 可选源按名字字母序；置顶仍按 pinOrder 段在前
	const available = $derived(all.filter((s) => !chosenIds.has(s.id)).sort(compareSourceName));
	const pinnedRest = $derived(
		all
			.filter((s) => s.pinned && !chosenIds.has(s.id))
			.sort((a, b) => (a.pinOrder ?? 0) - (b.pinOrder ?? 0))
	);

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

{#snippet rowIcon(source: Source)}
	{#if failed[source.id]}
		<span class="dest-picker__icon dest-picker__icon--fallback">{source.name.slice(0, 1)}</span>
	{:else}
		<span class="dest-picker__icon">
			<img
				src={sourceIconUrl(source.id)}
				alt=""
				loading="lazy"
				onerror={() => (failed = { ...failed, [source.id]: true })}
			/>
		</span>
	{/if}
{/snippet}

<div class="dest-picker">
	<div class="dest-picker__head">
		<h4>{$_('migrate.chosenHeading')}</h4>
		{#if pinnedRest.length > 0}
			<Button size="sm" onclick={addAllPinned} title={$_('migrate.addAllPinnedTitle')}>
				<ListPlus size={14} />
				{$_('migrate.addAllPinned', { values: { n: pinnedRest.length } })}
			</Button>
		{/if}
	</div>
	{#if chosen.length === 0}
		<p class="muted">{$_('migrate.chosenEmpty')}</p>
	{:else}
		<div class="dest-picker__list" role="list">
			{#each chosen as s, i (s.id)}
				<div
					class={[
						'dest-picker__row',
						dragIndex === i && 'dest-picker__row--dragging',
						overIndex === i && dragIndex !== null && dragIndex !== i && 'dest-picker__row--over'
					]}
					ondragover={(e) => {
						e.preventDefault();
						overIndex = i;
					}}
					ondrop={() => onDrop(i)}
					role="listitem"
				>
					<span
						class="dest-picker__grip"
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
						role="button"
						tabindex="-1"
						aria-label="drag"
					>
						<GripVertical size={15} />
					</span>
					<span class="dest-picker__order">{i + 1}</span>
					{@render rowIcon(s)}
					<span class="dest-picker__name">{s.name}</span>
					<button
						type="button"
						class="dest-picker__remove"
						title={$_('migrate.removeTitle', { values: { name: s.name } })}
						onclick={() => onChange(selected.filter((id) => id !== s.id))}
					>
						<X size={15} />
					</button>
				</div>
			{/each}
		</div>
	{/if}

	<h4>{$_('migrate.availableHeading')}</h4>
	{#if available.length === 0}
		<p class="muted">{$_('migrate.availableEmpty')}</p>
	{:else}
		<div class="dest-picker__list">
			{#each available as s (s.id)}
				<button
					type="button"
					class="dest-picker__row dest-picker__row--add"
					title={$_('migrate.addTitle', { values: { name: s.name } })}
					onclick={() => onChange([...selected, s.id])}
				>
					{@render rowIcon(s)}
					<span class="dest-picker__name">{s.name}</span>
					{#if s.pinned}<span class="dest-picker__pin">{$_('migrate.pinned')}</span>{/if}
					<Plus size={15} />
				</button>
			{/each}
		</div>
	{/if}
</div>

<style>
	.dest-picker h4 {
		margin: 12px 0 8px;
		font-size: 13px;
		color: var(--text-dim);
	}

	.dest-picker__head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
	}

	.dest-picker__list {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.dest-picker__row {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 8px;
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elev);
	}

	.dest-picker__row--dragging {
		opacity: 0.5;
	}

	.dest-picker__row--over {
		border-color: var(--accent);
	}

	.dest-picker__row--add {
		cursor: pointer;
		color: var(--text);
		font: inherit;
		text-align: left;
		width: 100%;
	}

	.dest-picker__row--add:hover {
		background: var(--bg-hover);
	}

	.dest-picker__grip {
		cursor: grab;
		color: var(--text-faint);
		display: inline-flex;
		touch-action: none;
	}

	.dest-picker__order {
		flex: none;
		width: 18px;
		text-align: center;
		font-size: 12px;
		color: var(--text-faint);
	}

	.dest-picker__icon {
		flex: none;
		width: 24px;
		height: 24px;
		border-radius: 6px;
		overflow: hidden;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		background: var(--bg-hover);
	}

	.dest-picker__icon img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.dest-picker__icon--fallback {
		font-size: 12px;
		color: var(--text-dim);
	}

	.dest-picker__name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.dest-picker__pin {
		flex: none;
		font-size: 11px;
		color: var(--accent);
	}

	.dest-picker__remove {
		flex: none;
		display: inline-flex;
		border: none;
		background: none;
		color: var(--text-faint);
		cursor: pointer;
		padding: 2px;
	}

	.dest-picker__remove:hover {
		color: var(--danger);
	}
</style>
