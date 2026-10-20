<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Download from '@lucide/svelte/icons/download';
	import FileUp from '@lucide/svelte/icons/file-up';
	import Upload from '@lucide/svelte/icons/upload';
	import { backupUrl } from '$lib/api/client';
	import { useImportAidoku, useImportSuwayomi, useRestoreBackup } from '$lib/api/hooks';
	import type {
		AidokuImportResult,
		BackupRestoreResult,
		SuwayomiImportResult,
		SuwayomiSourceGroup
	} from '$lib/api/types';
	import { Button, Modal, pushToast } from '$lib/components';
	import Section from './Section.svelte';
	import BackupResultView from './BackupResultView.svelte';
	import AidokuImportResultView from './AidokuImportResultView.svelte';
	import SuwayomiImportResultView from './SuwayomiImportResultView.svelte';
	import SuwayomiMappingSelect from './SuwayomiMappingSelect.svelte';

	const BACKUP_SECTIONS = [
		{ key: 'library', label: 'settings.sectionLibrary' },
		{ key: 'categories', label: 'settings.sectionCategories' },
		{ key: 'progress', label: 'settings.sectionProgress' },
		{ key: 'settings', label: 'settings.sectionSettings' },
		{ key: 'sourceSettings', label: 'settings.sectionSourceSettings' }
	] as const;

	interface BackupPreview {
		doc: unknown;
		createdAt: number | null;
		counts: Record<string, number>;
	}

	function previewCounts(doc: Record<string, unknown>): Record<string, number> {
		const count = (key: string): number => {
			const val = doc[key];
			if (Array.isArray(val)) return val.length;
			if (val && typeof val === 'object') return Object.keys(val).length;
			return 0;
		};
		const sourceSettings = doc['sourceSettings'];
		const credentialCount =
			sourceSettings && typeof sourceSettings === 'object'
				? Object.values(sourceSettings).reduce(
						(n, val) => n + (val && typeof val === 'object' ? Object.keys(val).length : 0),
						0
					)
				: 0;
		return {
			library: count('library'),
			categories: count('categories'),
			progress: count('progress'),
			settings: count('settings'),
			sourceSettings: credentialCount
		};
	}

	function groupName(g: SuwayomiSourceGroup): string {
		return g.mihonSourceName || $_('settings.unknownSourceNamed', { values: { id: g.mihonSourceId } });
	}

	const restore = useRestoreBackup();
	const importAidoku = useImportAidoku();
	const importSuwayomi = useImportSuwayomi();

	let fileEl: HTMLInputElement;
	let aibFileEl: HTMLInputElement;
	let swFileEl: HTMLInputElement;

	let preview = $state<BackupPreview | null>(null);
	let picked = $state<string[]>([]);
	let result = $state<BackupRestoreResult | null>(null);

	let aib = $state<{ file: File; preview: AidokuImportResult } | null>(null);
	let aibResult = $state<AidokuImportResult | null>(null);

	let sw = $state<{ file: File; preview: SuwayomiImportResult } | null>(null);
	let swMapping = $state<Record<string, string>>({});
	let swResult = $state<SuwayomiImportResult | null>(null);

	async function onSwFile(e: Event & { currentTarget: HTMLInputElement }) {
		const file = e.currentTarget.files?.[0];
		e.currentTarget.value = '';
		if (!file) return;
		try {
			const dry = await importSuwayomi.mutateAsync({ file, dryRun: true });
			sw = { file, preview: dry };
			swMapping = {};
			swResult = null;
		} catch {
			// api() 已弹错误提示
		}
	}

	function setSwSource(mihonId: string, key: string) {
		const nextMap = { ...swMapping };
		if (key) nextMap[mihonId] = key;
		else delete nextMap[mihonId];
		swMapping = nextMap;
	}

	async function confirmSwImport() {
		if (!sw) return;
		try {
			swResult = await importSuwayomi.mutateAsync({ file: sw.file, dryRun: false, mapping: swMapping });
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeSwModal() {
		sw = null;
		swMapping = {};
		swResult = null;
	}

	async function onAibFile(e: Event & { currentTarget: HTMLInputElement }) {
		const file = e.currentTarget.files?.[0];
		e.currentTarget.value = '';
		if (!file) return;
		try {
			const dryPreview = await importAidoku.mutateAsync({ file, dryRun: true });
			aib = { file, preview: dryPreview };
			aibResult = null;
		} catch {
			// api() 已弹错误提示
		}
	}

	async function confirmAibImport() {
		if (!aib) return;
		try {
			aibResult = await importAidoku.mutateAsync({ file: aib.file, dryRun: false });
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeAibModal() {
		aib = null;
		aibResult = null;
	}

	async function onFile(e: Event & { currentTarget: HTMLInputElement }) {
		const file = e.currentTarget.files?.[0];
		// 清空 value 让同一文件能再次触发 change
		e.currentTarget.value = '';
		if (!file) return;
		let doc: unknown;
		try {
			doc = JSON.parse(await file.text());
		} catch {
			pushToast($_('settings.backupInvalidJson'));
			return;
		}
		const version = (doc as { version?: unknown } | null)?.version;
		if (version !== 1) {
			pushToast(
				$_('settings.backupUnsupportedVersion', {
					values: { version: typeof version === 'number' ? version : $_('settings.backupVersionMissing') }
				})
			);
			return;
		}
		const createdAt = (doc as { createdAt?: unknown }).createdAt;
		preview = {
			doc,
			createdAt: typeof createdAt === 'number' ? createdAt : null,
			counts: previewCounts(doc as Record<string, unknown>)
		};
		picked = BACKUP_SECTIONS.map((s) => s.key);
		result = null;
	}

	function toggle(key: string, on: boolean) {
		picked = on ? [...picked, key] : picked.filter((k) => k !== key);
	}

	async function confirmRestore() {
		if (!preview || picked.length === 0) return;
		try {
			result = await restore.mutateAsync({ doc: preview.doc, sections: picked });
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeModal() {
		preview = null;
		result = null;
	}
</script>

<Section title={$_('settings.backupTitle')} desc={$_('settings.backupDesc')}>
	<div class="settings-backup__ops">
		<a class="btn settings-backup__download" href={backupUrl()} download>
			<Download size={15} />
			{$_('settings.downloadBackup')}
		</a>
		<Button onclick={() => fileEl.click()}>
			<Upload size={15} />
			{$_('settings.uploadRestore')}
		</Button>
		<input bind:this={fileEl} type="file" accept="application/json,.json" hidden onchange={onFile} />
		<Button loading={importAidoku.isPending && !aib} onclick={() => aibFileEl.click()}>
			<FileUp size={15} />
			{$_('settings.importAidoku')}
		</Button>
		<input bind:this={aibFileEl} type="file" accept=".aib" hidden onchange={onAibFile} />
		<Button loading={importSuwayomi.isPending && !sw} onclick={() => swFileEl.click()}>
			<FileUp size={15} />
			{$_('settings.importSuwayomi')}
		</Button>
		<input bind:this={swFileEl} type="file" accept=".tachibk,.gz,.proto" hidden onchange={onSwFile} />
	</div>

	<Modal open={sw !== null} title={$_('settings.importSuwayomi')} onClose={closeSwModal}>
		{#snippet footer()}
			{#if swResult}
				<Button variant="primary" onclick={closeSwModal}>{$_('settings.done')}</Button>
			{:else}
				<Button onclick={closeSwModal}>{$_('common.cancel')}</Button>
				<Button variant="primary" loading={importSuwayomi.isPending} onclick={() => void confirmSwImport()}>
					{$_('settings.confirmImport')}
				</Button>
			{/if}
		{/snippet}
		{#if swResult}
			<SuwayomiImportResultView result={swResult} />
		{:else if sw}
			<p class="settings-modal-text">
				{$_('settings.willImportSummary', {
					values: {
						library: sw.preview.imported.library,
						categories: sw.preview.imported.categories,
						progress: sw.preview.imported.progress
					}
				})}
			</p>
			<p class="settings-modal-text">{$_('settings.swMappingHead')}</p>
			<ul class="settings-backup__mapping">
				{#each sw.preview.sources as g (g.mihonSourceId)}
					<li class="settings-backup__mapping-row">
						<span class="settings-backup__mapping-name">
							{$_('settings.swGroupCount', { values: { name: groupName(g), n: g.mangaCount } })}
						</span>
						{#if g.match === 'unmatched'}
							<SuwayomiMappingSelect
								group={g}
								value={swMapping[g.mihonSourceId] ?? ''}
								onChange={(key) => setSwSource(g.mihonSourceId, key)}
							/>
						{:else}
							<span class="faint">
								{$_('settings.swAutoMatch', {
									values: { target: g.aidokuSourceName ?? g.aidokuSourceKey ?? '' }
								})}
							</span>
						{/if}
					</li>
				{/each}
			</ul>
			{#if sw.preview.skipped.length > 0}
				<p class="settings-modal-text">
					{$_('settings.swSkipNote', { values: { n: sw.preview.skipped.length } })}
				</p>
			{/if}
			<p class="settings-backup__warn">{$_('settings.swImportWarn')}</p>
		{/if}
	</Modal>

	<Modal open={aib !== null} title={$_('settings.importAidoku')} onClose={closeAibModal}>
		{#snippet footer()}
			{#if aibResult}
				<Button variant="primary" onclick={closeAibModal}>{$_('settings.done')}</Button>
			{:else}
				<Button onclick={closeAibModal}>{$_('common.cancel')}</Button>
				<Button variant="primary" loading={importAidoku.isPending} onclick={() => void confirmAibImport()}>
					{$_('settings.confirmImport')}
				</Button>
			{/if}
		{/snippet}
		{#if aibResult}
			<AidokuImportResultView result={aibResult} />
		{:else if aib}
			<p class="settings-modal-text">
				{$_('settings.willImportSummary', {
					values: {
						library: aib.preview.imported.library,
						categories: aib.preview.imported.categories,
						progress: aib.preview.imported.progress
					}
				})}
			</p>
			{#if aib.preview.skipped.length > 0}
				<p class="settings-modal-text">{$_('settings.aibSkippedHead')}</p>
				<ul class="settings-backup__warnings">
					{#each aib.preview.skipped as s, i (i)}
						<li>
							{(s.section === 'library'
								? $_('settings.skippedLibraryItem')
								: $_('settings.skippedProgress')) +
								' ' +
								s.sourceKey +
								'/' +
								s.mangaKey +
								(s.chapterKey
									? ' / ' + $_('settings.chapterPart', { values: { chapter: s.chapterKey } })
									: '')}
						</li>
					{/each}
				</ul>
			{/if}
			<p class="settings-backup__warn">{$_('settings.importMergeWarn')}</p>
		{/if}
	</Modal>

	<Modal open={preview !== null} title={$_('settings.restoreTitle')} onClose={closeModal}>
		{#snippet footer()}
			{#if result}
				<Button variant="primary" onclick={closeModal}>{$_('settings.done')}</Button>
			{:else}
				<Button onclick={closeModal}>{$_('common.cancel')}</Button>
				<Button
					variant="danger"
					loading={restore.isPending}
					disabled={picked.length === 0}
					onclick={() => void confirmRestore()}
				>
					{$_('settings.importAction')}
				</Button>
			{/if}
		{/snippet}
		{#if result}
			<BackupResultView {result} />
		{:else if preview}
			<p class="settings-modal-text">
				{preview.createdAt
					? $_('settings.restoreCreatedAt', {
							values: { date: new Date(preview.createdAt * 1000).toLocaleString() }
						})
					: $_('settings.restoreChoose')}
			</p>
			<div class="settings-backup__sections">
				{#each BACKUP_SECTIONS as s (s.key)}
					<label class="settings-backup__section">
						<input
							type="checkbox"
							checked={picked.includes(s.key)}
							onchange={(e) => toggle(s.key, e.currentTarget.checked)}
						/>
						{$_('settings.labeledCount', {
							values: { label: $_(s.label), n: preview.counts[s.key] ?? 0 }
						})}
					</label>
				{/each}
			</div>
			<p class="settings-backup__warn">{$_('settings.restoreWarn')}</p>
		{/if}
	</Modal>
</Section>
