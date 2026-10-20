<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { ChipDropdown } from '$lib/components';
	import type { Filter } from '$lib/api/types';
	import type { FilterStates, SortVal } from './filterState';
	import ChipMenuItem from './ChipMenuItem.svelte';

	interface Props {
		filter: Filter;
		states: FilterStates;
		setStates: (s: FilterStates) => void;
		onApply: (s: FilterStates) => void;
	}
	let { filter: f, states, setStates, onApply }: Props = $props();

	const options = $derived(f.options ?? []);
	const cur = $derived(states.sortVals[f.id] ?? { index: null, ascending: true });
	const def = $derived(f.default as { index?: number; ascending?: boolean } | null | undefined);
	// 与默认（未选视为 null/true）不同即启用
	const active = $derived(
		cur.index !== null &&
			(cur.index !== (def?.index ?? null) || cur.ascending !== (def?.ascending ?? true))
	);
	const label = $derived(
		cur.index !== null && cur.index < options.length
			? `${f.title ? `${f.title}: ` : ''}${options[cur.index]}${f.can_ascend ? (cur.ascending ? ' ↑' : ' ↓') : ''}`
			: (f.title ?? $_('sourceBrowse.sortLabel'))
	);

	function pick(index: number, close: () => void) {
		// Aidoku：点当前项且可升降序时切方向，否则选中并归默认方向
		const next: SortVal =
			cur.index === index && f.can_ascend
				? { index, ascending: !cur.ascending }
				: { index, ascending: false };
		const nextStates = { ...states, sortVals: { ...states.sortVals, [f.id]: next } };
		setStates(nextStates);
		onApply(nextStates);
		close();
	}
</script>

<ChipDropdown {active} {label}>
	{#snippet children(close)}
		{#each options as opt, i (i)}
			<ChipMenuItem checked={cur.index === i} onclick={() => pick(i, close)}>
				{opt}{cur.index === i && f.can_ascend ? (cur.ascending ? ' ↑' : ' ↓') : ''}
			</ChipMenuItem>
		{/each}
	{/snippet}
</ChipDropdown>
