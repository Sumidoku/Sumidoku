<script lang="ts">
	import type { Snippet } from 'svelte';
	import Check from '@lucide/svelte/icons/check';
	import X from '@lucide/svelte/icons/x';

	interface Props {
		checked?: boolean;
		excluded?: boolean;
		onclick: () => void;
		children: Snippet;
	}
	let { checked = false, excluded = false, onclick, children }: Props = $props();
</script>

<button type="button" class="chip-dropdown__item" {onclick}>
	<span>{@render children()}</span>
	{#if checked}<Check size={14} />{/if}
	{#if excluded}<X size={14} />{/if}
</button>
