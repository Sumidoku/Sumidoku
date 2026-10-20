<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { Button } from '$lib/components';
	import LoginLogoutRow from './LoginLogoutRow.svelte';

	/**
	 * OAuth 登录：授权页在新标签打开（URL 由服务端构建，pkce 时带 challenge）；
	 * 浏览器拦不到自定义 scheme 的重定向，用户授权后手动粘贴地址栏的回调串完成登录
	 */
	interface Props {
		title: string;
		logoutTitle: string | null;
		clearCookiesOnLogOut: boolean;
		loggedIn: boolean;
		/** 拿授权 URL（服务端构建）并新标签打开 */
		onStart: () => Promise<{ url: string }>;
		onComplete: (url: string) => Promise<void>;
		onLogout: (clearCookies: boolean) => Promise<void>;
	}
	let { title, logoutTitle, clearCookiesOnLogOut, loggedIn, onStart, onComplete, onLogout }: Props =
		$props();

	let text = $state('');
	let busy = $state(false);
	let startBusy = $state(false);
</script>

{#if loggedIn}
	<LoginLogoutRow {title} {logoutTitle} {clearCookiesOnLogOut} {onLogout} />
{:else}
	<div class="setting-row setting-row--col">
		<div class="setting-login__head">
			<span class="setting-row__title">{title}</span>
		</div>
		<span class="faint">{$_('settings.oauthHint')}</span>
		<Button
			size="sm"
			loading={startBusy}
			onclick={() => {
				// window.open 在 await 之后会被弹窗拦截（Safari 必拦）：同步先开空白
				// 标签，授权 URL 回来再导航它；同步阶段就被拦则退化为直接打开
				const tab = window.open('', '_blank');
				startBusy = true;
				void onStart()
					.then((data) => {
						if (tab && !tab.closed) {
							tab.location.href = data.url;
						} else {
							window.open(data.url, '_blank', 'noreferrer');
						}
					})
					.catch(() => tab?.close())
					.finally(() => (startBusy = false));
			}}
		>
			{$_('settings.login')}
		</Button>
		<textarea
			class="setting-textarea"
			rows="2"
			placeholder={$_('settings.oauthRedirectPlaceholder')}
			bind:value={text}
		></textarea>
		<Button
			size="sm"
			loading={busy}
			disabled={!text.trim()}
			onclick={() => {
				busy = true;
				void onComplete(text.trim()).finally(() => (busy = false));
			}}
		>
			{$_('settings.done')}
		</Button>
	</div>
{/if}
