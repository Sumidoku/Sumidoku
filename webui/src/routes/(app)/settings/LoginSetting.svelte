<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { Button, Chip, Input } from '$lib/components';

	/** 基本登录表单：已登录时回填用户名并标记登录态，提交后由源校验，成功即写回凭据 */
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

	let username = $state(savedUsername);
	let password = $state('');
	let busy = $state(false);
</script>

<div class="setting-row setting-row--col">
	<div class="setting-login__head">
		<span class="setting-row__title">{title}</span>
		{#if loggedIn}<Chip color="success">{$_('settings.loggedIn')}</Chip>{/if}
	</div>
	<Input
		placeholder={useEmail ? $_('settings.email') : $_('settings.username')}
		autocomplete={useEmail ? 'email' : 'username'}
		bind:value={username}
	/>
	<Input
		type="password"
		placeholder={$_('login.password')}
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
		{loggedIn ? $_('settings.relogin') : $_('settings.login')}
	</Button>
</div>
