<script lang="ts">
	import { _, locale } from 'svelte-i18n';
	import Languages from '@lucide/svelte/icons/languages';
	import Check from '@lucide/svelte/icons/check';
	import { Button } from '$lib/components/ui/button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import { LOCALES, LOCALE_LABELS, setLocale, type Locale } from '$lib/i18n';

	const current = $derived(($locale ?? 'en').split('-')[0] as Locale);
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="ghost" size="icon" aria-label={$_('locale.label')} title={$_('locale.label')}>
				<Languages size={17} />
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="end">
		{#each LOCALES as loc (loc)}
			<DropdownMenu.Item onSelect={() => setLocale(loc)}>
				<Check size={15} class={current === loc ? 'opacity-100' : 'opacity-0'} />
				<span>{LOCALE_LABELS[loc]}</span>
			</DropdownMenu.Item>
		{/each}
	</DropdownMenu.Content>
</DropdownMenu.Root>
