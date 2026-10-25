<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { useRegisterInternalSource } from '$lib/api/hooks';
	import type { RegisterInternalRequest } from '$lib/api/types';
	import { Button, Input, Modal, SegmentedControl, pushToast } from '$lib/components';
	import LocalDirsEditor from './LocalDirsEditor.svelte';

	type InternalKind = 'opds' | 'local';

	interface Props {
		open: boolean;
		onClose: () => void;
	}
	let { open, onClose }: Props = $props();

	const register = useRegisterInternalSource();

	let kind = $state<InternalKind>('opds');
	let name = $state('');
	// opds 字段
	let url = $state('');
	let username = $state('');
	let password = $state('');
	// local 字段
	let dirs = $state<string[]>(['']);
	let oneshots = $state('');

	function reset() {
		kind = 'opds';
		name = '';
		url = '';
		username = '';
		password = '';
		dirs = [''];
		oneshots = '';
	}

	const localPaths = $derived(dirs.map((d) => d.trim()).filter(Boolean));
	const canSubmit = $derived(
		name.trim() !== '' && (kind === 'opds' ? url.trim() !== '' : localPaths.length > 0)
	);

	async function submit() {
		const body: RegisterInternalRequest =
			kind === 'opds'
				? {
						type: 'opds',
						name: name.trim(),
						url: url.trim(),
						// 用户名留空表示公共目录，连同密码一起省略
						...(username.trim() ? { username: username.trim(), password } : {})
					}
				: {
						type: 'local',
						name: name.trim(),
						paths: localPaths,
						// 仅在填写时携带：缺省继承服务端 oneshots 配置
						...(oneshots.trim() ? { oneshotsDir: oneshots.trim() } : {})
					};
		try {
			const source = await register.mutateAsync(body);
			pushToast($_('browse.internalAdded', { values: { name: source.name } }), 'info');
			reset();
			onClose();
		} catch {
			// client 已弹错误 toast
		}
	}
</script>

<Modal {open} title={$_('browse.addInternal')} {onClose}>
	<div class="internal-form">
		<SegmentedControl
			options={[
				{ value: 'opds', label: $_('browse.internalTypeOpds') },
				{ value: 'local', label: $_('browse.internalTypeLocal') }
			]}
			value={kind}
			onChange={(v) => (kind = v as InternalKind)}
		/>
		<Input placeholder={$_('browse.opdsNamePlaceholder')} bind:value={name} />
		{#if kind === 'opds'}
			<Input placeholder={$_('browse.opdsUrlPlaceholder')} bind:value={url} />
			<Input placeholder={$_('browse.opdsUsernamePlaceholder')} autocomplete="off" bind:value={username} />
			<Input
				type="password"
				placeholder={$_('browse.opdsPasswordPlaceholder')}
				autocomplete="new-password"
				bind:value={password}
			/>
			<p class="muted">{$_('browse.opdsHint')}</p>
		{:else}
			<LocalDirsEditor {dirs} onChange={(d) => (dirs = d)} />
			<Input placeholder={$_('browse.localOneshotsPlaceholder')} bind:value={oneshots} />
			<p class="muted">{$_('browse.localHint')}</p>
		{/if}
	</div>
	{#snippet footer()}
		<Button onclick={onClose}>{$_('common.cancel')}</Button>
		<Button variant="primary" loading={register.isPending} disabled={!canSubmit} onclick={() => void submit()}>
			{$_('browse.add')}
		</Button>
	{/snippet}
</Modal>

<style>
	.internal-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
</style>
