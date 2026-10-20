<script lang="ts" generics="T extends string | number">
	import type { Snippet } from 'svelte';

	interface Option {
		value: T;
		label: string | Snippet;
	}
	interface Props {
		options: Option[];
		value: T;
		onChange: (v: T) => void;
	}
	let { options, value, onChange }: Props = $props();

	let items: (HTMLButtonElement | undefined)[] = $state([]);
	let thumb = $state<{ left: number; width: number } | null>(null);

	$effect(() => {
		const i = options.findIndex((o) => o.value === value);
		const el = i >= 0 ? items[i] : undefined;
		thumb = el ? { left: el.offsetLeft, width: el.offsetWidth } : null;
	});
</script>

<div class="seg" role="tablist">
	{#if thumb}
		<span class="seg__thumb" style={`left:${thumb.left}px;width:${thumb.width}px`}></span>
	{/if}
	{#each options as o, i (o.value)}
		<button
			type="button"
			role="tab"
			aria-selected={o.value === value}
			bind:this={items[i]}
			class={['seg__item', o.value === value && 'seg__item--active']}
			onclick={() => onChange(o.value)}
		>
			{#if typeof o.label === 'string'}{o.label}{:else}{@render o.label()}{/if}
		</button>
	{/each}
</div>
