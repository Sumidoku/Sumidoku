<script lang="ts">
	import type { Snippet } from 'svelte';
	interface Props {
		title: string | Snippet;
		actions?: Snippet;
	}
	let { title, actions }: Props = $props();
</script>

<header class="page-header">
	<h1 class="page-header__title">
		{#if typeof title === 'string'}{title}{:else}{@render title()}{/if}
	</h1>
	{#if actions}<div class="page-header__actions">{@render actions()}</div>{/if}
</header>
