<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { ChipDropdown } from '$lib/components';
	import type { Filter } from '$lib/api/types';
	import { optionKey, type FilterStates } from './filterState';
	import ChipMenuItem from './ChipMenuItem.svelte';

	interface Props {
		filter: Filter;
		states: FilterStates;
		setStates: (s: FilterStates) => void;
		onApply: (s: FilterStates) => void;
	}
	let { filter: f, states, setStates, onApply }: Props = $props();

	const cur = $derived(states.selectVals[f.id] ?? '');
	const defVal = $derived(typeof f.default === 'string' ? f.default : '');
	const active = $derived(cur !== '' && cur !== defVal);

	function pick(value: string, close: () => void) {
		const nextStates = { ...states, selectVals: { ...states.selectVals, [f.id]: value } };
		setStates(nextStates);
		onApply(nextStates);
		close();
	}
</script>

<ChipDropdown {active} label={f.title ?? ''}>
	{#snippet children(close)}
		{#if cur !== ''}
			<ChipMenuItem checked onclick={() => pick('', close)}>
				{$_('sourceBrowse.selectNone')}
			</ChipMenuItem>
		{/if}
		{#each f.options ?? [] as opt, i (i)}
			{@const key = optionKey(f, i)}
			<ChipMenuItem checked={cur === key} onclick={() => pick(key, close)}>
				{opt}
			</ChipMenuItem>
		{/each}
	{/snippet}
</ChipDropdown>
