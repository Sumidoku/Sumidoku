<script lang="ts">
	import type { Component, Snippet } from 'svelte';

	interface Props {
		icon?: Component<{ size?: number; strokeWidth?: number }>;
		title: string;
		hint?: string;
		action?: Snippet;
	}
	let { icon: Icon, title, hint, action }: Props = $props();
</script>

<div class="empty-state">
	{#if Icon}
		<div class="empty-state__icon"><Icon size={40} strokeWidth={1.5} /></div>
	{/if}
	<div class="empty-state__title">{title}</div>
	{#if hint}<div class="empty-state__hint">{hint}</div>{/if}
	{#if action}<div class="empty-state__action">{@render action()}</div>{/if}
</div>
