<script lang="ts">
	import { _ } from 'svelte-i18n';
	import type { AidokuImportResult, AidokuImportSkipped } from '$lib/api/types';

	let { result }: { result: AidokuImportResult } = $props();

	function skippedLabel(s: AidokuImportSkipped): string {
		const what = s.section === 'library' ? $_('settings.skippedLibraryItem') : $_('settings.skippedProgress');
		const chapter = s.chapterKey ? ` / ${$_('settings.chapterPart', { values: { chapter: s.chapterKey } })}` : '';
		return `${what} ${s.sourceKey}/${s.mangaKey}${chapter}`;
	}
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
	{#if result.skipped.length === 0}
		<p class="faint">{$_('settings.importedAll')}</p>
	{:else}
		<p class="settings-modal-text">{$_('settings.importedSkippedHead')}</p>
		<ul class="settings-backup__warnings">
			{#each result.skipped as s, i (i)}
				<li>{skippedLabel(s)}</li>
			{/each}
		</ul>
	{/if}
</div>
