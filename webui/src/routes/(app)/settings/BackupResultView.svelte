<script lang="ts">
	import { _ } from 'svelte-i18n';
	import type { BackupRestoreResult } from '$lib/api/types';

	let { result }: { result: BackupRestoreResult } = $props();
</script>

<div class="settings-backup__result">
	<p class="settings-modal-text">
		{$_('settings.restoredSummary', {
			values: {
				library: result.restored.library,
				categories: result.restored.categories,
				progress: result.restored.progress,
				settings: result.restored.settings,
				sourceSettings: result.restored.sourceSettings
			}
		})}
	</p>
	{#if result.warnings.length === 0}
		<p class="faint">{$_('settings.restoredAll')}</p>
	{:else}
		<p class="settings-modal-text">{$_('settings.restoredSkippedHead')}</p>
		<ul class="settings-backup__warnings">
			{#each result.warnings as w, i (i)}
				<li>{w}</li>
			{/each}
		</ul>
	{/if}
</div>
