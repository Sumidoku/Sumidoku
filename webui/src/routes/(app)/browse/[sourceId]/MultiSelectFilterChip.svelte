<script lang="ts">
	import { ChipDropdown } from '$lib/components';
	import type { Filter } from '$lib/api/types';
	import { nextMultiVal, optionKey, type CheckVal, type FilterStates } from './filterState';
	import ChipMenuItem from './ChipMenuItem.svelte';

	interface Props {
		filter: Filter;
		states: FilterStates;
		setStates: (s: FilterStates) => void;
		onApply: (s: FilterStates) => void;
	}
	let { filter: f, states, setStates, onApply }: Props = $props();

	const rec = $derived(states.multiVals[f.id] ?? {});
	const count = $derived(Object.keys(rec).length);

	function toggle(i: number) {
		const key = optionKey(f, i);
		if (!key) return;
		const nextRec: Record<string, CheckVal> = { ...rec };
		const next = nextMultiVal(nextRec[key] ?? 0, !!f.can_exclude);
		if (next === 0) delete nextRec[key];
		else nextRec[key] = next;
		const nextStates = { ...states, multiVals: { ...states.multiVals, [f.id]: nextRec } };
		setStates(nextStates);
		onApply(nextStates);
		// Aidoku menuActionDismissDisabled：菜单不收起，连续多选
	}
</script>

<ChipDropdown active={count > 0} label={`${f.title ?? ''}${count > 0 ? `（${count}）` : ''}`}>
	{#snippet children()}
		{#each f.options ?? [] as opt, i (i)}
			{@const key = optionKey(f, i)}
			{@const st = rec[key] ?? 0}
			<ChipMenuItem checked={st === 1} excluded={st === -1} onclick={() => toggle(i)}>
				{st === -1 ? '− ' : ''}{opt}
			</ChipMenuItem>
		{/each}
	{/snippet}
</ChipDropdown>
