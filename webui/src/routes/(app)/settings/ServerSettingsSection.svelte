<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { useServerSettings, useSaveServerSettings } from '$lib/api/hooks';
	import { Button, IconButton, Input, Skeleton, pushToast } from '$lib/components';
	import Section from './Section.svelte';

	interface KvRow {
		key: string;
		/** JSON 文本，保存时再解析 */
		text: string;
		isNew?: boolean;
	}

	function parseVal(text: string): unknown {
		try {
			return JSON.parse(text);
		} catch {
			// 不是合法 JSON 时按字符串存
			return text;
		}
	}

	const query = useServerSettings();
	const save = useSaveServerSettings();

	let rows = $state<KvRow[] | null>(null);
	let newKey = $state('');
	let newVal = $state('');

	$effect(() => {
		if (query.data && rows === null) {
			rows = Object.entries(query.data).map(([key, v]) => ({ key, text: JSON.stringify(v) ?? '' }));
		}
	});

	async function onSave() {
		if (!rows) return;
		const body: Record<string, unknown> = {};
		for (const r of rows) {
			const k = r.key.trim();
			if (k) body[k] = parseVal(r.text);
		}
		const saved = await save.mutateAsync(body);
		rows = Object.entries(saved).map(([key, v]) => ({ key, text: JSON.stringify(v) ?? '' }));
		pushToast($_('settings.serverSaved'), 'info');
	}

	function addRow() {
		const key = newKey.trim();
		if (!key) return;
		rows = [...(rows ?? []), { key, text: newVal, isNew: true }];
		newKey = '';
		newVal = '';
	}
</script>

<Section title={$_('settings.serverTitle')} desc={$_('settings.serverDesc')}>
	{#if rows === null}
		<Skeleton height={48} />
	{:else}
		{#if rows.length === 0}
			<p class="faint settings-kv__empty">{$_('settings.kvEmpty')}</p>
		{:else}
			<div class="settings-kv">
				{#each rows as row, i (i)}
					<div class="settings-kv__row">
						<Input
							class={row.isNew ? '' : 'settings-kv__key'}
							placeholder={$_('settings.kvKey')}
							bind:value={row.key}
							readonly={!row.isNew}
						/>
						<Input placeholder={$_('settings.kvValuePlaceholder')} bind:value={row.text} />
						<IconButton
							label={$_('common.delete')}
							variant="danger"
							onclick={() => (rows = (rows ?? []).filter((_, j) => j !== i))}
						>
							<Trash2 size={16} />
						</IconButton>
					</div>
				{/each}
			</div>
		{/if}
		<div class="settings-kv__add">
			<Input placeholder={$_('settings.kvKey')} bind:value={newKey} />
			<Input placeholder={$_('settings.kvValuePlaceholder')} bind:value={newVal} />
			<Button disabled={!newKey.trim()} onclick={addRow}>{$_('common.add')}</Button>
		</div>
		<div class="settings-kv__save">
			<Button variant="primary" loading={save.isPending} onclick={() => void onSave()}>
				{$_('common.save')}
			</Button>
		</div>
	{/if}
</Section>
