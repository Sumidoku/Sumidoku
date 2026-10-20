<script lang="ts">
	import { _ } from 'svelte-i18n';
	import type { BrowserLoginSession } from '$lib/api/types';
	import { Button, Input, pushToast } from '$lib/components';
	import BrowserLoginViewer from './BrowserLoginViewer.svelte';
	import SettingLoginLogoutRow from './SettingLoginLogoutRow.svelte';

	interface Props {
		title: string;
		/** 登录页地址（设置声明的 url），提示用户去哪里登录 */
		url: string | null;
		logoutTitle: string | null;
		localStorageKeys: string[];
		clearCookiesOnLogOut: boolean;
		loggedIn: boolean;
		sourceId: number;
		onSubmit: (
			cookies: Record<string, string>,
			localStorage: Record<string, string>
		) => Promise<void>;
		onLogout: (clearCookies: boolean) => Promise<void>;
		browserLogin?: boolean;
		onStartBrowserLogin?: () => Promise<BrowserLoginSession>;
		onCompleteBrowserLogin?: (session: string) => Promise<void>;
		onCancelBrowserLogin?: (session: string) => Promise<void>;
	}
	let {
		title,
		url,
		logoutTitle,
		localStorageKeys,
		clearCookiesOnLogOut,
		loggedIn,
		sourceId,
		onSubmit,
		onLogout,
		browserLogin,
		onStartBrowserLogin,
		onCompleteBrowserLogin,
		onCancelBrowserLogin
	}: Props = $props();

	let text = $state('');
	let lsValues = $state<Record<string, string>>({});
	let busy = $state(false);
	let browserSession = $state<BrowserLoginSession | null>(null);
	let browserBusy = $state(false);

	const canBrowserLogin = $derived(
		browserLogin && onStartBrowserLogin && onCompleteBrowserLogin && onCancelBrowserLogin
	);

	/**
	 * 解析粘贴的 cookie：接受 `Cookie:` 请求头（name=value; …）、
	 * `{name: value}` JSON 对象，或浏览器导出的 [{name, value, …}] 数组
	 */
	function parseCookies(input: string): Record<string, string> | null {
		const trimmed = input.trim().replace(/^cookie:\s*/i, '');
		if (!trimmed) return null;
		if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
			try {
				const parsed: unknown = JSON.parse(trimmed);
				const entries = Array.isArray(parsed) ? parsed : [parsed];
				const out: Record<string, string> = {};
				for (const entry of entries) {
					if (!entry || typeof entry !== 'object') return null;
					if (Array.isArray(parsed)) {
						const { name, value } = entry as { name?: unknown; value?: unknown };
						if (typeof name !== 'string' || !name || value === undefined) return null;
						out[name] = String(value);
					} else {
						for (const [k, v] of Object.entries(entry)) out[k] = String(v);
					}
				}
				return Object.keys(out).length > 0 ? out : null;
			} catch {
				return null;
			}
		}
		const out: Record<string, string> = {};
		for (const part of trimmed.split(';')) {
			const eq = part.indexOf('=');
			const name = eq > 0 ? part.slice(0, eq).trim() : '';
			if (!name) return null;
			out[name] = part.slice(eq + 1).trim();
		}
		return out;
	}

	async function closeBrowserSession(cancel: boolean) {
		const sess = browserSession;
		browserSession = null;
		if (sess && cancel && onCancelBrowserLogin) {
			await onCancelBrowserLogin(sess.session).catch(() => {});
		}
	}
</script>

{#if loggedIn}
	<SettingLoginLogoutRow {title} {logoutTitle} {clearCookiesOnLogOut} {onLogout} />
{:else}
	<div class="setting-row setting-row--col">
		<div class="setting-login__head">
			<span class="setting-row__title">{title}</span>
		</div>
		{#if canBrowserLogin}
			<span class="faint">{$_('browse.browserLoginHint')}</span>
			<Button
				size="sm"
				loading={browserBusy}
				onclick={() => {
					browserBusy = true;
					void onStartBrowserLogin!()
						.then((s) => (browserSession = s))
						.finally(() => (browserBusy = false));
				}}
			>
				{$_('browse.openBrowserLogin')}
			</Button>
		{/if}
		<span class="faint">
			{#if url}
				{$_('browse.webLoginHintPre')}<a href={url} target="_blank" rel="noreferrer">{url}</a
				>{$_('browse.webLoginHintPost')}
			{:else}
				{$_('browse.webLoginHintNoUrl')}
			{/if}
		</span>
		<textarea class="setting-textarea" rows="3" placeholder={$_('browse.cookiePlaceholder')} bind:value={text}
		></textarea>
		{#each localStorageKeys as lsKey (lsKey)}
			<Input
				placeholder={$_('browse.localStoragePlaceholder', { values: { key: lsKey } })}
				value={lsValues[lsKey] ?? ''}
				oninput={(e) => {
					lsValues = { ...lsValues, [lsKey]: e.currentTarget.value };
				}}
			/>
		{/each}
		<Button
			size="sm"
			loading={busy}
			disabled={!text.trim()}
			onclick={() => {
				const cookies = parseCookies(text);
				if (!cookies) {
					pushToast($_('browse.cookieParseError'), 'error');
					return;
				}
				busy = true;
				void onSubmit(cookies, lsValues).finally(() => (busy = false));
			}}
		>
			{$_('browse.login')}
		</Button>
		{#if browserSession && onCompleteBrowserLogin}
			<BrowserLoginViewer
				{sourceId}
				session={browserSession.session}
				loginUrl={browserSession.loginUrl}
				onComplete={async () => {
					await onCompleteBrowserLogin!(browserSession!.session);
					await closeBrowserSession(false);
				}}
				onCancel={() => void closeBrowserSession(true)}
			/>
		{/if}
	</div>
{/if}
