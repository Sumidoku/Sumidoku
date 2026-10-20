<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { useSources } from '$lib/api/hooks';
	import type { SuwayomiSourceGroup } from '$lib/api/types';
	import { Select } from '$lib/components';

	/** unmatched 组的映射下拉：同名候选排前，其余已装源在后；留空即整组跳过 */
	interface Props {
		group: SuwayomiSourceGroup;
		value: string;
		onChange: (key: string) => void;
	}
	let { group, value, onChange }: Props = $props();

	const sources = useSources();
	const candidateKeys = $derived(new Set(group.candidates.map((c) => c.key)));
	const rest = $derived((sources.data ?? []).filter((s) => !candidateKeys.has(s.key)));
</script>

<Select
	{value}
	onchange={(e) => {
		const val = e.currentTarget.value;
		onChange(val);
	}}
>
	<option value="">{$_('settings.skipThisSource')}</option>
	{#each group.candidates as c (c.key)}
		<option value={c.key}>{$_('settings.sourceWithKey', { values: { name: c.name, key: c.key } })}</option>
	{/each}
	{#each rest as s (s.key)}
		<option value={s.key}>{$_('settings.sourceWithKey', { values: { name: s.name, key: s.key } })}</option>
	{/each}
</Select>
