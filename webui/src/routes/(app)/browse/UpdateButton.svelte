<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import CircleArrowUp from '@lucide/svelte/icons/circle-arrow-up';
	import type { Source } from '$lib/api/types';
	import { useUpdateSource } from '$lib/api/hooks';
	import { Button, pushToast } from '$lib/components';

	interface Props {
		source: Source;
	}
	let { source }: Props = $props();

	const update = useUpdateSource();

	async function onClick(e: MouseEvent) {
		e.stopPropagation();
		try {
			const r = await update.mutateAsync(source.id);
			pushToast($_('browse.updatedToast', { values: { name: source.name, version: r.version } }), 'info');
			for (const w of r.migrationWarnings.slice(0, 3)) pushToast(w, 'info');
			if (r.migrationRequired) {
				pushToast($_('browse.migrationNeeded', { values: { name: source.name } }), 'info');
				goto(`/migrate?source=${source.id}&sameSource=1`);
			}
		} catch {
			// client 已弹错误 toast
		}
	}
</script>

<Button size="sm" variant="primary" loading={update.isPending} onclick={(e) => void onClick(e)}>
	<CircleArrowUp size={14} />
	v{source.updateVersion}
</Button>
