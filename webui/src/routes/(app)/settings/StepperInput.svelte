<script lang="ts">
	import { untrack } from 'svelte';
	import { Input } from '$lib/components';

	interface Props {
		value: number;
		min: number;
		max: number;
		step: number;
		onCommit: (n: number) => void;
	}
	let { value, min, max, step, onCommit }: Props = $props();

	/** number 输入允许中间态（清空、负号），失焦才夹回 min/max */
	let text = $state(untrack(() => String(value)));
	$effect(() => {
		text = String(value);
	});
</script>

<Input
	class="setting-stepper__input"
	type="number"
	{min}
	{max}
	{step}
	value={text}
	oninput={(e) => {
		const raw = e.currentTarget.value;
		text = raw;
		const n = Number(raw);
		if (raw !== '' && Number.isFinite(n)) onCommit(n);
	}}
	onblur={() => {
		const n = Number(text);
		const clamped = text !== '' && Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : value;
		text = String(clamped);
		if (clamped !== value) onCommit(clamped);
	}}
/>
