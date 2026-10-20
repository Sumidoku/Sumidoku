<script lang="ts">
	import { untrack } from 'svelte';
	import { _ } from 'svelte-i18n';
	import { Button, Chip, Input } from '$lib/components';

	interface Props {
		title: string;
		useEmail: boolean;
		/** 服务端已存的用户名，回填输入框 */
		savedUsername: string;
		/** 服务端已标记登录 */
		loggedIn: boolean;
		onSubmit: (username: string, password: string) => Promise<void>;
	}
	let { title, useEmail, savedUsername, loggedIn, onSubmit }: Props = $props();

	let username = $state(untrack(() => savedUsername));
	let password = $state('');
	let busy = $state(false);
</script>

<div class="setting-row setting-row--col">
	<div class="setting-login__head">
		<span class="setting-row__title">{title}</span>
		{#if loggedIn}<Chip color="success">{$_('browse.loggedIn')}</Chip>{/if}
	</div>
	<Input
		placeholder={useEmail ? $_('browse.email') : $_('browse.username')}
		autocomplete={useEmail ? 'email' : 'username'}
		bind:value={username}
	/>
	<Input
		type="password"
		placeholder={$_('browse.password')}
		autocomplete="current-password"
		bind:value={password}
	/>
	<Button
		size="sm"
		loading={busy}
		disabled={!username || !password}
		onclick={() => {
			busy = true;
			void onSubmit(username, password).finally(() => (busy = false));
		}}
	>
		{loggedIn ? $_('browse.reLogin') : $_('browse.login')}
	</Button>
</div>
