<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Check from '@lucide/svelte/icons/check';
	import X from '@lucide/svelte/icons/x';
	import { ChipDropdown } from '$lib/components';
	import type { TriState } from '$lib/globalSearchPrefs';

	interface Props {
		label: string;
		options: { key: string; label: string }[];
		states: Record<string, TriState>;
		onCycle: (key: string) => void;
	}
	let { label, options, states, onCycle }: Props = $props();

	const count = $derived(Object.keys(states).length);
	// chip 展示选中计数；菜单项循环 未选 → 包含 → 排除，菜单不收起（连续多选）
	const chipLabel = $derived(
		count > 0 ? $_('search.filterCount', { values: { label, n: count } }) : label
	);
</script>

<ChipDropdown active={count > 0} label={chipLabel}>
	{#snippet children(_close)}
		{#each options as opt (opt.key)}
			{@const st = states[opt.key] ?? 0}
			<button type="button" class="chip-dropdown__item" onclick={() => onCycle(opt.key)}>
				<span>{#if st === -1}−{' '}{/if}{opt.label}</span>
				{#if st === 1}<Check size={14} />{/if}
				{#if st === -1}<X size={14} />{/if}
			</button>
		{/each}
	{/snippet}
</ChipDropdown>
