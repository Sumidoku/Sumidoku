<script lang="ts">
	import Check from '@lucide/svelte/icons/check';
	import X from '@lucide/svelte/icons/x';
	import { Chip } from '$lib/components';
	import type { Filter } from '$lib/api/types';
	import { nextMultiVal, type FilterStates } from './filterState';

	interface Props {
		filter: Filter;
		states: FilterStates;
		setStates: (s: FilterStates) => void;
		onApply: (s: FilterStates) => void;
	}
	let { filter: f, states, setStates, onApply }: Props = $props();

	const cur = $derived(states.checkVals[f.id] ?? 0);

	function toggle() {
		const nextStates = {
			...states,
			checkVals: { ...states.checkVals, [f.id]: nextMultiVal(cur, !!f.can_exclude) }
		};
		setStates(nextStates);
		onApply(nextStates);
	}
</script>

<Chip color={cur !== 0 ? 'accent' : 'default'} onclick={toggle}>
	{#if f.can_exclude && cur === 1}<Check size={13} />{/if}
	{#if f.can_exclude && cur === -1}<X size={13} />{/if}
	{f.name ?? f.title ?? ''}
</Chip>
