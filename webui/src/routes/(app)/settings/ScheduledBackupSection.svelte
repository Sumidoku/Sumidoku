<script lang="ts">
	import { _ } from 'svelte-i18n';
	import RotateCw from '@lucide/svelte/icons/rotate-cw';
	import {
		useBackupFiles,
		useRestoreNamedBackup,
		useSaveServerSettings,
		useServerSettings
	} from '$lib/api/hooks';
	import type { BackupFileEntry, BackupRestoreResult } from '$lib/api/types';
	import { Button, IconButton, Input, Modal, Skeleton, pushToast } from '$lib/components';
	import Section from './Section.svelte';
	import BackupResultView from './BackupResultView.svelte';

	function formatSize(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
	}

	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const backups = useBackupFiles();
	const restore = useRestoreNamedBackup();

	let confirming = $state<BackupFileEntry | null>(null);
	let result = $state<BackupRestoreResult | null>(null);
	let intervalText = $state('');
	let retentionText = $state('');
	let seeded = $state(false);

	$effect(() => {
		if (!settings.data || seeded) return;
		intervalText = String(Number(settings.data['backup.interval_hours'] ?? 24));
		retentionText = String(Number(settings.data['backup.retention_count'] ?? 10));
		seeded = true;
	});

	const data = $derived(settings.data);
	const ready = $derived(!!data && seeded);

	const enabled = $derived(data?.['backup.enabled'] === true);
	const lastRun = $derived(data?.['backup.last_run']);
	const files = $derived(backups.data?.files ?? []);
	const intervalHours = $derived(Number(data?.['backup.interval_hours'] ?? 24));
	const retentionCount = $derived(Number(data?.['backup.retention_count'] ?? 10));

	function commitNumber(
		key: 'backup.interval_hours' | 'backup.retention_count',
		text: string,
		fallback: number,
		min: number,
		reset: (v: string) => void
	) {
		const n = Number(text);
		if (!Number.isInteger(n) || n < min) {
			pushToast($_('settings.intervalInvalid', { values: { min } }));
			reset(String(fallback));
			return;
		}
		if (n !== fallback) save.mutate({ [key]: n });
	}

	async function confirmRestore() {
		if (!confirming) return;
		try {
			result = await restore.mutateAsync(confirming.name);
			confirming = null;
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeModal() {
		confirming = null;
		result = null;
	}

	const lastRunText = $derived(
		typeof lastRun === 'number' ? new Date(lastRun * 1000).toLocaleString() : $_('settings.never')
	);
</script>

<Section title={$_('settings.scheduledTitle')} desc={$_('settings.scheduledDesc')}>
	{#if !ready}
		<Skeleton height={48} />
	{:else}
		<label class="settings-backup__section">
			<input
				type="checkbox"
				checked={enabled}
				onchange={(e) => save.mutate({ 'backup.enabled': e.currentTarget.checked })}
			/>
			{$_('settings.enableScheduled')}
		</label>
		<div class="settings-row">
			<span class="settings-row__label">{$_('settings.intervalHours')}</span>
			<Input
				class="settings-backup__num"
				type="number"
				min={1}
				bind:value={intervalText}
				onblur={() =>
					commitNumber('backup.interval_hours', intervalText, intervalHours, 1, (v) => (intervalText = v))}
				onkeydown={(e) => {
					if (e.key === 'Enter')
						commitNumber('backup.interval_hours', intervalText, intervalHours, 1, (v) => (intervalText = v));
				}}
			/>
		</div>
		<div class="settings-row">
			<span class="settings-row__label">{$_('settings.retentionCount')}</span>
			<Input
				class="settings-backup__num"
				type="number"
				min={0}
				bind:value={retentionText}
				onblur={() =>
					commitNumber('backup.retention_count', retentionText, retentionCount, 0, (v) => (retentionText = v))}
				onkeydown={(e) => {
					if (e.key === 'Enter')
						commitNumber('backup.retention_count', retentionText, retentionCount, 0, (v) => (retentionText = v));
				}}
			/>
		</div>
		<div class="settings-row">
			<span class="settings-row__label">{$_('settings.backupDir')}</span>
			<span class="settings-backup__dir">{backups.data?.dir ?? '…'}</span>
		</div>
		<p class="faint settings-backup__hint">
			{$_('settings.backupDirHint', { values: { last: lastRunText } })}
		</p>

		<div class="settings-backup__files-head">
			<span class="settings-row__label">{$_('settings.existingBackups')}</span>
			<IconButton label={$_('settings.refresh')} size="sm" onclick={() => void backups.refetch()}>
				<RotateCw size={14} />
			</IconButton>
		</div>
		{#if files.length === 0}
			<p class="faint">{$_('settings.noBackups')}</p>
		{:else}
			<div class="settings-backup__files">
				{#each files as f (f.name)}
					<div class="settings-backup__file">
						<div class="settings-backup__file-meta">
							<span>{new Date(f.createdAt * 1000).toLocaleString()}</span>
							<span class="faint">{formatSize(f.size)}</span>
						</div>
						<Button size="sm" onclick={() => (confirming = f)}>{$_('settings.restoreAction')}</Button>
					</div>
				{/each}
			</div>
		{/if}

		<Modal
			open={confirming !== null || result !== null}
			title={$_('settings.restoreTitle')}
			onClose={closeModal}
		>
			{#snippet footer()}
				{#if result}
					<Button variant="primary" onclick={closeModal}>{$_('settings.done')}</Button>
				{:else}
					<Button onclick={closeModal}>{$_('common.cancel')}</Button>
					<Button variant="danger" loading={restore.isPending} onclick={() => void confirmRestore()}>
						{$_('settings.restoreAction')}
					</Button>
				{/if}
			{/snippet}
			{#if result}
				<BackupResultView {result} />
			{:else if confirming}
				<p class="settings-modal-text">
					{$_('settings.namedRestoreConfirm', {
						values: { date: new Date(confirming.createdAt * 1000).toLocaleString() }
					})}
				</p>
				<p class="settings-backup__warn">{$_('settings.credentialsWarn')}</p>
			{/if}
		</Modal>
	{/if}
</Section>
