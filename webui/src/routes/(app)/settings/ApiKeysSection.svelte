<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Copy from '@lucide/svelte/icons/copy';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { ApiError } from '$lib/api/client';
	import { useApiKeys, useCreateApiKey, useRevokeApiKey } from '$lib/api/hooks';
	import type { ApiKey, ApiKeyCreated } from '$lib/api/types';
	import { Button, EmptyState, IconButton, Input, Modal, Skeleton, pushToast } from '$lib/components';
	import { formatDate } from '$lib/format';
	import Section from './Section.svelte';

	const keys = useApiKeys();
	const create = useCreateApiKey();
	const revoke = useRevokeApiKey();

	let newName = $state('');
	let created = $state<ApiKeyCreated | null>(null);
	let revoking = $state<ApiKey | null>(null);

	const entries = $derived(keys.data ?? []);

	function toastError(e: unknown) {
		if (e instanceof ApiError) pushToast(e.message || $_('settings.operationFailed'));
		else pushToast(e instanceof Error ? e.message : $_('settings.operationFailed'));
	}

	async function submitCreate() {
		const name = newName.trim();
		if (create.isPending || !name) return;
		try {
			const entry = await create.mutateAsync(name);
			newName = '';
			created = entry;
		} catch (e) {
			toastError(e);
		}
	}

	function dismissCreated() {
		create.reset();
		created = null;
	}

	async function copyCreated() {
		if (!created) return;
		try {
			await navigator.clipboard.writeText(created.key);
			pushToast($_('settings.keyCopied'), 'info');
		} catch {
			pushToast($_('settings.operationFailed'));
		}
	}
</script>

<Section title={$_('settings.apiKeysTitle')} desc={$_('settings.apiKeysDesc')}>
	{#if keys.isPending}
		<div class="settings-cats">
			<Skeleton height={48} />
		</div>
	{:else if entries.length === 0}
		<EmptyState title={$_('settings.apiKeysEmpty')} hint={$_('settings.apiKeysEmptyHint')} />
	{:else}
		<div class="settings-cats">
			{#each entries as entry (entry.id)}
				<div class="settings-cat">
					<div class="settings-cat__head">
						<span class="settings-cat__name">{entry.name}</span>
						<span class="settings-cat__ops">
							<IconButton
								label={$_('settings.revoke')}
								size="sm"
								variant="danger"
								onclick={() => (revoking = entry)}
							>
								<Trash2 size={15} />
							</IconButton>
						</span>
					</div>
					<p class="faint">
						{$_('settings.createdAt')}
						{formatDate(entry.createdAt)}
						{' · '}
						{$_('settings.lastUsed')}
						{entry.lastUsedAt === null ? $_('settings.never') : formatDate(entry.lastUsedAt)}
					</p>
				</div>
			{/each}
		</div>
	{/if}

	<div class="settings-cat__create">
		<Input
			placeholder={$_('settings.apiKeyNamePlaceholder')}
			bind:value={newName}
			disabled={create.isPending}
			onkeydown={(e) => {
				if (e.key === 'Enter') void submitCreate();
			}}
		/>
		<Button
			variant="primary"
			loading={create.isPending}
			disabled={create.isPending || !newName.trim()}
			onclick={() => void submitCreate()}
		>
			{$_('settings.createApiKey')}
		</Button>
	</div>

	<Modal open={created !== null} title={$_('settings.apiKeyCreatedTitle')} onClose={dismissCreated}>
		{#snippet footer()}
			<Button variant="primary" onclick={dismissCreated}>{$_('settings.done')}</Button>
		{/snippet}
		<p class="settings-modal-text">{$_('settings.apiKeyCreatedHint')}</p>
		<div class="settings-cat__create">
			<Input
				class="settings-modal-input"
				readonly
				value={created?.key ?? ''}
				onfocus={(e) => e.currentTarget.select()}
			/>
			<Button onclick={() => void copyCreated()}>
				<Copy size={15} />
				{$_('settings.copyKey')}
			</Button>
		</div>
	</Modal>

	<Modal open={revoking !== null} title={$_('settings.revoke')} onClose={() => (revoking = null)}>
		{#snippet footer()}
			<Button onclick={() => (revoking = null)}>{$_('common.cancel')}</Button>
			<Button
				variant="danger"
				loading={revoke.isPending}
				onclick={() => {
					if (!revoking) return;
					revoke.mutate(revoking.id, {
						onSuccess: () => {
							revoking = null;
							pushToast($_('settings.apiKeyRevoked'), 'info');
						},
						onError: toastError
					});
				}}
			>
				{$_('settings.revoke')}
			</Button>
		{/snippet}
		<p class="settings-modal-text">
			{$_('settings.revokeApiKeyConfirm', { values: { name: revoking?.name ?? '' } })}
		</p>
	</Modal>
</Section>
