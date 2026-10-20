<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { useServerSettings, useSaveServerSettings } from '$lib/api/hooks';
	import { Skeleton, Switch } from '$lib/components';
	import Section from './Section.svelte';

	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const enabled = $derived(settings.data?.['incognito.enabled'] === true);
</script>

<Section title={$_('settings.incognitoTitle')} desc={$_('settings.incognitoDesc')}>
	{#if settings.isPending}
		<Skeleton height={48} />
	{:else}
		<label class="settings-backup__section">
			<Switch
				checked={enabled}
				disabled={save.isPending}
				onChange={(on) => save.mutate({ 'incognito.enabled': on })}
				label={$_('settings.incognitoGlobal')}
			/>
			{$_('settings.incognitoGlobal')}
		</label>
	{/if}
</Section>
