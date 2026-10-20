<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import CircleArrowUp from '@lucide/svelte/icons/circle-arrow-up';
	import Download from '@lucide/svelte/icons/download';
	import type { RepoSourceEntry } from '$lib/api/types';
	import { useInstallRepoSource, useUpdateSource } from '$lib/api/hooks';
	import { Button, pushToast } from '$lib/components';

	interface Props {
		entry: RepoSourceEntry;
	}
	let { entry }: Props = $props();

	const install = useInstallRepoSource();
	const update = useUpdateSource();
	const busy = $derived(install.isPending || update.isPending);

	async function onClick() {
		try {
			if (entry.installedId !== null && entry.updateVersion !== null) {
				const r = await update.mutateAsync(entry.installedId);
				pushToast($_('browse.updatedToast', { values: { name: entry.name, version: r.version } }), 'info');
				for (const w of r.migrationWarnings.slice(0, 3)) pushToast(w, 'info');
				if (r.migrationRequired) {
					pushToast($_('browse.migrationNeeded', { values: { name: entry.name } }), 'info');
					goto(`/migrate?source=${entry.installedId}&sameSource=1`);
				}
			} else if (entry.installedId === null) {
				await install.mutateAsync({ repoId: entry.repoId, sourceKey: entry.key });
				pushToast($_('browse.installedToast', { values: { name: entry.name } }), 'info');
			}
		} catch {
			// client 已弹错误 toast
		}
	}
</script>

{#if entry.installedId !== null && entry.updateVersion !== null}
	<Button variant="primary" loading={busy} onclick={() => void onClick()}>
		<CircleArrowUp size={15} />
		{$_('browse.updateVersionBtn', { values: { version: entry.updateVersion } })}
	</Button>
{:else if entry.installedId !== null}
	<Button disabled>
		{$_('browse.installedVersionBtn', { values: { version: entry.installedVersion } })}
	</Button>
{:else}
	<Button variant="primary" loading={busy} onclick={() => void onClick()}>
		<Download size={15} />
		{$_('browse.install')}
	</Button>
{/if}
