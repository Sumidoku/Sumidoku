<script lang="ts">
	import { untrack } from 'svelte';
	import { _ } from 'svelte-i18n';
	import {
		queryClient,
		invalidateForRefreshes,
		useBasicLogin,
		useCancelBrowserLogin,
		useClearSourceCache,
		useCompleteBrowserLogin,
		useLoginLogout,
		useNotifySource,
		useOAuthLoginComplete,
		useOAuthLoginStart,
		useResetSourceSettings,
		useSaveSourceSettings,
		useSourceSettings,
		useStartBrowserLogin,
		useWebLogin
	} from '$lib/api/hooks';
	import type { Setting } from '$lib/api/types';
	import { Button, Modal, Spinner, pushToast } from '$lib/components';
	import SettingItem from './SettingItem.svelte';

	interface Props {
		sourceId: number;
	}
	let { sourceId }: Props = $props();

	// 面板按源挂载（模态每次为某个源打开），sourceId 在实例生命周期内不变；
	// 这些变更 hook 工厂只需一次性拿到 id，用 untrack 明确「取初值」意图
	const sid = untrack(() => sourceId);

	const query = useSourceSettings(() => sourceId);
	const save = useSaveSourceSettings(sid);
	const reset = useResetSourceSettings(sid);
	const clearCache = useClearSourceCache(sid);
	const login = useBasicLogin(sid);
	const webLogin = useWebLogin(sid);
	const startBrowserLogin = useStartBrowserLogin(sid);
	const completeBrowserLogin = useCompleteBrowserLogin(sid);
	const cancelBrowserLogin = useCancelBrowserLogin(sid);
	const loginLogout = useLoginLogout(sid);
	const oauthStart = useOAuthLoginStart(sid);
	const oauthComplete = useOAuthLoginComplete(sid);
	const notifySource = useNotifySource(sid);

	let edits = $state<Record<string, unknown>>({});
	let pending = $state<'reset' | 'clear' | null>(null);

	const data = $derived(query.data);
	const hasEdits = $derived(Object.keys(edits).length > 0);

	/** 摊平 group/page 里的嵌套设置，按 key 找配置项用 */
	function flattenSettings(list: Setting[]): Setting[] {
		return list.flatMap((s) => {
			const v = s.value;
			if (typeof v === 'object' && 'Group' in v) return flattenSettings(v.Group.items);
			if (typeof v === 'object' && 'Page' in v) return flattenSettings(v.Page.items);
			return [s];
		});
	}

	async function onSave() {
		if (!hasEdits || !data?.settings) return;
		const keys = Object.keys(edits);
		const touched = flattenSettings(data.settings).filter((s) => keys.includes(s.key));
		// 每个被改设置声明的 notification 都发给源（Aidoku：值一变即发）
		const notify = touched
			.map((s) => s.notification)
			.filter((n): n is string => typeof n === 'string' && n !== '');
		await save.mutateAsync({ edits, notify });
		edits = {};
		pushToast($_('browse.saved'), 'info');
		invalidateForRefreshes(
			queryClient,
			sourceId,
			touched.flatMap((s) => s.refreshes ?? [])
		);
	}

	/** Button 设置按下即把它的 notification 发给源，再按声明刷新相关数据 */
	async function onPressButton(setting: Setting) {
		if (!setting.notification) return;
		await notifySource.mutateAsync(setting.notification);
		invalidateForRefreshes(queryClient, sourceId, setting.refreshes ?? []);
	}

	async function onReset() {
		await reset.mutateAsync();
		edits = {};
		pending = null;
		pushToast($_('browse.resetDone'), 'info');
	}

	async function onClearCache() {
		await clearCache.mutateAsync();
		pending = null;
		pushToast($_('browse.cacheCleared'), 'info');
	}

	async function onSubmitLogin(key: string, username: string, password: string) {
		await login.mutateAsync({ key, username, password });
		edits = {};
		pushToast($_('browse.loginSuccess'), 'info');
	}

	async function onSubmitWebLogin(
		key: string,
		cookies: Record<string, string>,
		localStorage: Record<string, string>
	) {
		await webLogin.mutateAsync({ key, cookies, localStorage });
		edits = {};
		pushToast($_('browse.loginSuccess'), 'info');
	}

	async function onCompleteBrowserLogin(key: string, session: string) {
		await completeBrowserLogin.mutateAsync({ key, session });
		edits = {};
		pushToast($_('browse.loginSuccess'), 'info');
	}

	async function onCancelBrowserLogin(key: string, session: string) {
		await cancelBrowserLogin.mutateAsync({ key, session });
	}

	async function onLogoutLogin(key: string, clearCookies: boolean) {
		await loginLogout.mutateAsync({ key, clearCookies });
		edits = {};
		pushToast($_('browse.logoutDone'), 'info');
	}

	async function onCompleteOAuth(key: string, url: string) {
		await oauthComplete.mutateAsync({ key, url });
		edits = {};
		pushToast($_('browse.loginSuccess'), 'info');
	}
</script>

{#if query.isPending}
	<div class="settings-source__loading">
		<Spinner /> {$_('common.loading')}
	</div>
{:else if !data}
	<p class="faint settings-source__empty">{$_('browse.settingsLoadFailed')}</p>
{:else if !data.settings || data.settings.length === 0}
	<p class="faint settings-source__empty">{$_('browse.noSettings')}</p>
{:else}
	<div class="settings-source__body">
		{#each data.settings as setting (setting.key)}
			<SettingItem
				{setting}
				{sourceId}
				values={data.values}
				{edits}
				onEdit={(k, val) => (edits = { ...edits, [k]: val })}
				{onPressButton}
				{onSubmitLogin}
				{onSubmitWebLogin}
				{onLogoutLogin}
				browserLogin={data.capabilities?.browserLogin === true}
				onStartBrowserLogin={(key) => startBrowserLogin.mutateAsync(key)}
				{onCompleteBrowserLogin}
				{onCancelBrowserLogin}
				onStartOAuth={(key) => oauthStart.mutateAsync(key)}
				{onCompleteOAuth}
			/>
		{/each}
		<div class="settings-source__save">
			<Button variant="ghost" size="sm" onclick={() => (pending = 'reset')}>
				{$_('browse.resetSettings')}
			</Button>
			<Button variant="ghost" size="sm" onclick={() => (pending = 'clear')}>
				{$_('browse.clearCache')}
			</Button>
			<Button
				variant="primary"
				size="sm"
				loading={save.isPending}
				disabled={!hasEdits}
				onclick={() => void onSave()}
			>
				{$_('common.save')}
			</Button>
		</div>
		<Modal
			open={pending === 'reset'}
			title={$_('browse.resetSettings')}
			onClose={() => (pending = null)}
		>
			<p>{$_('browse.resetSettingsBody')}</p>
			{#snippet footer()}
				<Button onclick={() => (pending = null)}>{$_('common.cancel')}</Button>
				<Button variant="primary" loading={reset.isPending} onclick={() => void onReset()}>
					{$_('browse.reset')}
				</Button>
			{/snippet}
		</Modal>
		<Modal
			open={pending === 'clear'}
			title={$_('browse.clearCache')}
			onClose={() => (pending = null)}
		>
			<p>{$_('browse.clearCacheBody')}</p>
			{#snippet footer()}
				<Button onclick={() => (pending = null)}>{$_('common.cancel')}</Button>
				<Button variant="primary" loading={clearCache.isPending} onclick={() => void onClearCache()}>
					{$_('browse.clear')}
				</Button>
			{/snippet}
		</Modal>
	</div>
{/if}
