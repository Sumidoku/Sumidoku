<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { ApiError, api, clearToken } from '$lib/api/client';
	import { Button, Input, Skeleton, pushToast } from '$lib/components';
	import Section from './Section.svelte';

	let enabled = $state<boolean | null>(null);
	let current = $state('');
	let next = $state('');
	let pending = $state(false);

	$effect(() => {
		api<{ enabled: boolean }>('/auth/status', { silent: true })
			.then((s) => (enabled = s.enabled))
			.catch(() => (enabled = null));
	});

	async function submit() {
		if (!current || !next || pending) return;
		pending = true;
		try {
			// 改密码会吊销全部会话 token，成功后必须重新登录
			await api('/auth/password', {
				method: 'PUT',
				body: { currentPassword: current, newPassword: next },
				silent: true
			});
			pushToast($_('settings.passwordChanged'), 'info');
			clearToken();
			goto('/login');
		} catch (e) {
			pushToast(
				e instanceof ApiError && e.status === 401
					? $_('settings.currentPasswordWrong')
					: $_('settings.changePasswordFailed')
			);
		} finally {
			pending = false;
		}
	}

	async function logout() {
		await api('/auth/logout', { method: 'POST', silent: true }).catch(() => {});
		clearToken();
		goto('/login');
	}
</script>

<Section title={$_('settings.securityTitle')} desc={$_('settings.securityDesc')}>
	{#if enabled === null}
		<Skeleton height={48} />
	{:else if !enabled}
		<p class="faint settings-security__off">{$_('settings.securityOff')}</p>
	{:else}
		<div class="settings-row">
			<span class="settings-row__label">{$_('settings.currentPassword')}</span>
			<Input type="password" bind:value={current} />
		</div>
		<div class="settings-row">
			<span class="settings-row__label">{$_('settings.newPassword')}</span>
			<Input type="password" bind:value={next} />
		</div>
		<div class="settings-security__ops">
			<Button variant="primary" loading={pending} disabled={!current || !next} onclick={() => void submit()}>
				{$_('settings.changePassword')}
			</Button>
			<Button onclick={() => void logout()}>{$_('settings.logout')}</Button>
		</div>
	{/if}
</Section>
