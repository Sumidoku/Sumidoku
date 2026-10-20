<script lang="ts">
	import { untrack } from 'svelte';
	import { _ } from 'svelte-i18n';
	import { Button, Chip, Modal, Switch } from '$lib/components';

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
</script>

<div class="setting-row setting-row--col">
	<div class="setting-login__head">
		<span class="setting-row__title">{title}</span>
		<Chip color="success">{$_('browse.loggedIn')}</Chip>
	</div>
	<Button
		size="sm"
		onclick={() => {
			clearCookies = clearCookiesOnLogOut;
			confirmOpen = true;
		}}
	>
		{logoutTitle ?? $_('browse.logout')}
	</Button>
	<Modal
		open={confirmOpen}
		title={logoutTitle ?? $_('browse.logout')}
		onClose={() => (confirmOpen = false)}
	>
		<p>{$_('browse.logoutConfirmBody')}</p>
		<label class="setting-row">
			<span class="setting-row__title">{$_('browse.alsoClearCookies')}</span>
			<Switch bind:checked={clearCookies} label={$_('browse.alsoClearCookies')} />
		</label>
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
				{logoutTitle ?? $_('browse.logout')}
			</Button>
		{/snippet}
	</Modal>
</div>
