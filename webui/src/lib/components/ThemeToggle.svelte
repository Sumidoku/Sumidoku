<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Sun from '@lucide/svelte/icons/sun';
	import Moon from '@lucide/svelte/icons/moon';
	import { Button } from '$lib/components/ui/button';
	import { resolvedTheme, setTheme } from '$lib/theme';

	const next = $derived($resolvedTheme === 'dark' ? 'light' : 'dark');
	const label = $derived(next === 'light' ? $_('theme.toLight') : $_('theme.toDark'));
</script>

<Button variant="ghost" size="icon" aria-label={label} title={label} onclick={() => setTheme(next)}>
	{#if $resolvedTheme === 'dark'}
		<Sun size={17} />
	{:else}
		<Moon size={17} />
	{/if}
</Button>
