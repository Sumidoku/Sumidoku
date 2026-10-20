<script lang="ts">
	import { _ } from 'svelte-i18n';
	import type {
		SuwayomiImportResult,
		SuwayomiImportSkipped,
		SuwayomiSourceGroup
	} from '$lib/api/types';

	let { result }: { result: SuwayomiImportResult } = $props();

	function groupName(g: SuwayomiSourceGroup): string {
		return g.mihonSourceName || $_('settings.unknownSourceNamed', { values: { id: g.mihonSourceId } });
	}

	function skippedLabel(s: SuwayomiImportSkipped): string {
		const what = s.section === 'library' ? $_('settings.skippedLibraryItem') : $_('settings.skippedProgress');
		const name = s.mihonSourceName || $_('settings.unknownSourceId', { values: { id: s.mihonSourceId } });
		return `${what} ${name}/${s.mangaKey}`;
	}

	const sourcesLine = $derived(
		result.sources
			.map((g) => {
				const label = groupName(g);
				if (g.match === 'unmatched')
					return $_('settings.swGroupUnmatched', { values: { label, n: g.mangaCount } });
				const how =
					g.match === 'auto' ? $_('settings.swMatchAuto') : $_('settings.swMatchManual');
				return $_('settings.swGroupMatched', {
					values: {
						label,
						n: g.mangaCount,
						how,
						target: g.aidokuSourceName ?? g.aidokuSourceKey ?? ''
					}
				});
			})
			.join($_('settings.listSeparator'))
	);
</script>

<div class="settings-backup__result">
	<p class="settings-modal-text">
		{$_('settings.importedSummary', {
			values: {
				library: result.imported.library,
				categories: result.imported.categories,
				progress: result.imported.progress
			}
		})}
	</p>
	<p class="settings-modal-text">{sourcesLine}</p>
	{#if result.skipped.length > 0}
		<p class="settings-modal-text">{$_('settings.restoredSkippedHead')}</p>
		<ul class="settings-backup__warnings">
			{#each result.skipped as s, i (i)}
				<li>{skippedLabel(s)}</li>
			{/each}
		</ul>
	{/if}
</div>
