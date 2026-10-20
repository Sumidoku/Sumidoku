<script lang="ts">
	import type { Filter } from '$lib/api/types';
	import { isFilterEnabled, type FilterStates } from './filterState';
	import SortFilterChip from './SortFilterChip.svelte';
	import CheckFilterChip from './CheckFilterChip.svelte';
	import SelectFilterChip from './SelectFilterChip.svelte';
	import MultiSelectFilterChip from './MultiSelectFilterChip.svelte';

	interface Props {
		filters: Filter[];
		states: FilterStates;
		setStates: (s: FilterStates) => void;
		onApply: (s: FilterStates) => void;
	}
	let { filters, states, setStates, onApply }: Props = $props();

	// 启用在先
	const sorted = $derived(
		[...filters].sort(
			(a, b) => Number(isFilterEnabled(b, states)) - Number(isFilterEnabled(a, states))
		)
	);
</script>

<div class="filter-header">
	{#each sorted as f (f.id)}
		{#if !(f.hide_from_header || f.type === 'note' || f.type === 'text' || f.type === 'range')}
			{#if f.type === 'sort'}
				<SortFilterChip filter={f} {states} {setStates} {onApply} />
			{:else if f.type === 'check'}
				<!-- Aidoku：有默认值的 check 不进头部（重置后不会误显示为启用） -->
				{#if f.default == null}
					<CheckFilterChip filter={f} {states} {setStates} {onApply} />
				{/if}
			{:else if f.type === 'select'}
				<SelectFilterChip filter={f} {states} {setStates} {onApply} />
			{:else if f.type === 'multi-select'}
				<MultiSelectFilterChip filter={f} {states} {setStates} {onApply} />
			{/if}
		{/if}
	{/each}
</div>
