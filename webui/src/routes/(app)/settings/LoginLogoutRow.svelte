<script lang="ts">
	import { untrack } from 'svelte';
	import { _ } from 'svelte-i18n';
	import { Button, Chip, Modal, Switch } from '$lib/components';

	/** 已登录态的行：标题 + 已登录 chip + 登出按钮与确认框（web/oauth 登录共用） */
	interface Props {
		title: string;
		logoutTitle: string | null;
		clearCookiesOnLogOut: boolean;
		onLogout: (clearCookies: boolean) => Promise<void>;
	}
	let { title, logoutTitle, clearCookiesOnLogOut, onLogout }: Props = $props();

	let busy = $state(false);
	let confirmOpen = $state(false);
	let clearCookies = $state(untrack(() => clearCookiesOnLogOut));

	const label = $derived(logoutTitle ?? $_('settings.logoutDefault'));
</script>

<div class="setting-row setting-row--col">
	<div class="setting-login__head">
		<span class="setting-row__title">{title}</span>
		<Chip color="success">{$_('settings.loggedIn')}</Chip>
	</div>
	<Button
		size="sm"
		onclick={() => {
			clearCookies = clearCookiesOnLogOut;
			confirmOpen = true;
		}}
	>
		{label}
	</Button>
	<Modal open={confirmOpen} title={label} onClose={() => (confirmOpen = false)}>
		{#snippet footer()}
			<Button onclick={() => (confirmOpen = false)}>{$_('common.cancel')}</Button>
			<Button
				variant="primary"
				loading={busy}
				onclick={() => {
					busy = true;
					void onLogout(clearCookies)
						.then(() => (confirmOpen = false))
						.finally(() => (busy = false));
				}}
			>
				{label}
			</Button>
		{/snippet}
		<p>{$_('settings.logoutConfirm')}</p>
		<label class="setting-row">
			<span class="setting-row__title">{$_('settings.clearLoginCookies')}</span>
			<Switch bind:checked={clearCookies} label={$_('settings.clearLoginCookies')} />
		</label>
	</Modal>
</div>
