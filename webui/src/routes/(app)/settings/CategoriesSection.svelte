<script lang="ts">
	import { _ } from 'svelte-i18n';
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { ApiError } from '$lib/api/client';
	import {
		useCategories,
		useCreateCategory,
		useDeleteCategory,
		usePatchCategory,
		useRenameCategory,
		useReorderCategories
	} from '$lib/api/hooks';
	import type { Category } from '$lib/api/types';
	import { Badge, Button, IconButton, Input, Modal, Select, Skeleton, pushToast } from '$lib/components';
	import Section from './Section.svelte';

	const categories = useCategories();
	const patch = usePatchCategory();
	const create = useCreateCategory();
	const rename = useRenameCategory();
	const del = useDeleteCategory();
	const reorder = useReorderCategories();

	let newName = $state('');
	let renaming = $state<Category | null>(null);
	let renameText = $state('');
	let deleting = $state<Category | null>(null);

	const cats = $derived(categories.data ?? []);

	const triState = $derived([
		{ value: 0, label: $_('settings.triFollowDefault') },
		{ value: 1, label: $_('settings.triOnly') },
		{ value: 2, label: $_('settings.triExclude') }
	]);

	function toastError(e: unknown) {
		if (e instanceof ApiError && e.status === 409) pushToast($_('settings.categoryExists'));
		else pushToast(e instanceof Error ? e.message : $_('settings.operationFailed'));
	}

	async function submitCreate() {
		const name = newName.trim();
		if (!name) return;
		try {
			await create.mutateAsync(name);
			newName = '';
			pushToast($_('settings.categoryCreated'), 'info');
		} catch (e) {
			toastError(e);
		}
	}

	async function submitRename() {
		if (!renaming) return;
		const name = renameText.trim();
		if (!name) return;
		try {
			await rename.mutateAsync({ id: renaming.id, name });
			renaming = null;
			pushToast($_('settings.categoryRenamed'), 'info');
		} catch (e) {
			toastError(e);
		}
	}

	function move(index: number, delta: number) {
		const ids = cats.map((c) => c.id);
		const j = index + delta;
		if (j < 0 || j >= ids.length) return;
		[ids[index], ids[j]] = [ids[j], ids[index]];
		reorder.mutate(ids);
	}
</script>

<Section title={$_('settings.categoriesTitle')} desc={$_('settings.categoriesDesc')}>
	{#if categories.isPending}
		<div class="settings-cats">
			{#each Array.from({ length: 2 }) as _item, i (i)}
				<Skeleton height={72} />
			{/each}
		</div>
	{:else}
		<div class="settings-cats">
			{#each cats as c, i (c.id)}
				<div class="settings-cat">
					<div class="settings-cat__head">
						<span class="settings-cat__name">
							{c.name}
							<Badge>{c.mangaCount}</Badge>
						</span>
						<span class="settings-cat__ops">
							<IconButton
								label={$_('settings.catRename')}
								size="sm"
								onclick={() => {
									renaming = c;
									renameText = c.name;
								}}
							>
								<Pencil size={15} />
							</IconButton>
							<IconButton
								label={$_('settings.catMoveUp')}
								size="sm"
								disabled={i === 0 || reorder.isPending}
								onclick={() => move(i, -1)}
							>
								<ArrowUp size={15} />
							</IconButton>
							<IconButton
								label={$_('settings.catMoveDown')}
								size="sm"
								disabled={i === cats.length - 1 || reorder.isPending}
								onclick={() => move(i, 1)}
							>
								<ArrowDown size={15} />
							</IconButton>
							{#if !c.isDefault}
								<IconButton
									label={$_('settings.catDelete')}
									size="sm"
									variant="danger"
									onclick={() => (deleting = c)}
								>
									<Trash2 size={15} />
								</IconButton>
							{/if}
						</span>
					</div>
					<div class="settings-cat__fields">
						<div class="settings-cat__field">
							<span class="settings-cat__label">{$_('settings.catUpdateLabel')}</span>
							<Select
								value={String(c.includeInUpdate)}
								onchange={(e) => {
									const val = Number(e.currentTarget.value);
									patch.mutate({ id: c.id, includeInUpdate: val });
								}}
							>
								{#each triState as o (o.value)}
									<option value={String(o.value)}>{o.label}</option>
								{/each}
							</Select>
							<span class="settings-cat__hint">{$_('settings.catUpdateHint')}</span>
						</div>
						<div class="settings-cat__field">
							<span class="settings-cat__label">{$_('settings.catDownloadLabel')}</span>
							<Select
								value={String(c.includeInDownload)}
								onchange={(e) => {
									const val = Number(e.currentTarget.value);
									patch.mutate({ id: c.id, includeInDownload: val });
								}}
							>
								{#each triState as o (o.value)}
									<option value={String(o.value)}>{o.label}</option>
								{/each}
							</Select>
							<span class="settings-cat__hint">{$_('settings.catDownloadHint')}</span>
						</div>
					</div>
				</div>
			{/each}
		</div>
	{/if}

	<div class="settings-cat__create">
		<Input
			placeholder={$_('settings.newCategoryPlaceholder')}
			bind:value={newName}
			onkeydown={(e) => {
				if (e.key === 'Enter') void submitCreate();
			}}
		/>
		<Button variant="primary" loading={create.isPending} disabled={!newName.trim()} onclick={() => void submitCreate()}>
			{$_('settings.createCategory')}
		</Button>
	</div>

	<Modal open={renaming !== null} title={$_('settings.renameCategoryTitle')} onClose={() => (renaming = null)}>
		{#snippet footer()}
			<Button onclick={() => (renaming = null)}>{$_('common.cancel')}</Button>
			<Button variant="primary" loading={rename.isPending} disabled={!renameText.trim()} onclick={() => void submitRename()}>
				{$_('common.save')}
			</Button>
		{/snippet}
		<Input
			class="settings-modal-input"
			placeholder={$_('settings.categoryNamePlaceholder')}
			bind:value={renameText}
			onkeydown={(e) => {
				if (e.key === 'Enter') void submitRename();
			}}
		/>
	</Modal>

	<Modal open={deleting !== null} title={$_('settings.deleteCategoryTitle')} onClose={() => (deleting = null)}>
		{#snippet footer()}
			<Button onclick={() => (deleting = null)}>{$_('common.cancel')}</Button>
			<Button
				variant="danger"
				loading={del.isPending}
				onclick={() => {
					if (!deleting) return;
					del.mutate(deleting.id, {
						onSuccess: () => {
							deleting = null;
							pushToast($_('settings.categoryDeleted'), 'info');
						}
					});
				}}
			>
				{$_('common.delete')}
			</Button>
		{/snippet}
		<p class="settings-modal-text">
			{$_('settings.deleteCategoryConfirm', { values: { name: deleting?.name ?? '' } })}
		</p>
	</Modal>
</Section>
