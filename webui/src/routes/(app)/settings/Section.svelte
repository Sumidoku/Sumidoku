<script lang="ts">
	import type { Snippet } from 'svelte';
	interface Props {
		title: string;
		desc?: string;
		children: Snippet;
	}
	let { title, desc, children }: Props = $props();
</script>

<section class="settings-section card">
	<h2 class="settings-section__title">{title}</h2>
	{#if desc}<p class="settings-section__desc">{desc}</p>{/if}
	<div class="settings-section__body">{@render children()}</div>
</section>
