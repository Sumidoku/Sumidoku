<script lang="ts">
	import SettingItem from './SettingItem.svelte';
	import { _ } from 'svelte-i18n';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import type { BrowserLoginSession, Setting } from '$lib/api/types';
	import { Button, Chip, Input, Modal, SegmentedControl, Select, Switch } from '$lib/components';
	import StepperInput from './StepperInput.svelte';
	import LoginSetting from './LoginSetting.svelte';
	import WebLoginSetting from './WebLoginSetting.svelte';
	import OAuthSetting from './OAuthSetting.svelte';
	import ButtonRow from './ButtonRow.svelte';
	import '$lib/styles/settings.css';

	export interface SettingItemProps {
		setting: Setting;
		/** 所属源 id：sidecar 交互登录的 WS 中继路径要用 */
		sourceId: number;
		/** 服务端已保存的值 */
		values: Record<string, unknown>;
		/** 本地未保存的改动，优先于 values 显示 */
		edits: Record<string, unknown>;
		onEdit: (key: string, value: unknown) => void;
		/** Button 设置按下：把它的 notification 发给源 */
		onPressButton?: (setting: Setting) => Promise<void>;
		/** 基本登录：由外层提供端点调用，缺省时 login 项降级为只读提示 */
		onSubmitLogin?: (key: string, username: string, password: string) => Promise<void>;
		/** 网页登录：粘贴 cookie 换取登录态 */
		onSubmitWebLogin?: (
			key: string,
			cookies: Record<string, string>,
			localStorage: Record<string, string>
		) => Promise<void>;
		/** 源级登出：clearCookies 为真时连登录 cookie 一起清 */
		onLogoutLogin?: (key: string, clearCookies: boolean) => Promise<void>;
		/** 服务端配了 sidecar（capabilities.browserLogin）时 web 登录项显示交互登录入口 */
		browserLogin?: boolean;
		/** sidecar 交互登录：开会话返回句柄与登录页地址 */
		onStartBrowserLogin?: (key: string) => Promise<BrowserLoginSession>;
		/** sidecar 交互登录：完成（收割 cookie）与取消（清理 context） */
		onCompleteBrowserLogin?: (key: string, session: string) => Promise<void>;
		onCancelBrowserLogin?: (key: string, session: string) => Promise<void>;
		/** OAuth 登录：拿授权 URL（新标签打开）与粘贴回调串完成 */
		onStartOAuth?: (key: string) => Promise<{ url: string }>;
		onCompleteOAuth?: (key: string, url: string) => Promise<void>;
	}

	let {
		setting,
		sourceId,
		values,
		edits,
		onEdit,
		onPressButton,
		onSubmitLogin,
		onSubmitWebLogin,
		onLogoutLogin,
		browserLogin,
		onStartBrowserLogin,
		onCompleteBrowserLogin,
		onCancelBrowserLogin,
		onStartOAuth,
		onCompleteOAuth
	}: SettingItemProps = $props();

	function strArr(v: unknown): string[] {
		return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
	}
	function num(v: unknown, fallback: number): number {
		return typeof v === 'number' && Number.isFinite(v) ? v : fallback;
	}

	let msOpen = $state(false);

	// 当前值优先取未保存的修改，其次取服务器已存值
	const cur = (key: string): unknown => (key in edits ? edits[key] : values[key]);

	// requires / requires_false 控制显隐，按当前值实时判断
	const visible = $derived(
		!(setting.requires && cur(setting.requires) !== true) &&
			!(setting.requires_false && cur(setting.requires_false) === true)
	);

	const v = $derived(setting.value);

	// Picker（wire 上与 Select 同构）宽松取值
	const picker = $derived(
		setting.type === 'picker'
			? (
					v as unknown as Record<
						string,
						| { values?: string[]; titles?: (string | null)[] | null; default?: string | null }
						| undefined
					>
				).Picker
			: undefined
	);
	const pickerOpts = $derived(picker?.values ?? []);
	const pickerSelected = $derived(
		typeof cur(setting.key) === 'string' ? (cur(setting.key) as string) : (picker?.default ?? '')
	);
</script>

{#if visible}
	{#if setting.type === 'editable-list'}
		<label class="setting-row setting-row--col">
			<span class="setting-row__title">{setting.title}</span>
			<textarea
				class="setting-textarea"
				rows="5"
				value={strArr(cur(setting.key)).join('\n')}
				oninput={(e) => {
					const val = e.currentTarget.value;
					onEdit(setting.key, val.split('\n'));
				}}
			></textarea>
		</label>
	{:else if setting.type === 'picker'}
		<div class="setting-row setting-row--col">
			<span class="setting-row__title">{setting.title}</span>
			<div class="setting-chips">
				{#each pickerOpts as opt, i (opt)}
					<Chip
						color={pickerSelected === opt ? 'accent' : 'default'}
						onclick={() => onEdit(setting.key, opt)}
					>
						{picker?.titles?.[i] ?? opt}
					</Chip>
				{/each}
			</div>
		</div>
	{:else if typeof v === 'object' && 'Login' in v}
		{#if onSubmitLogin && v.Login.method === 'basic'}
			<LoginSetting
				title={setting.title}
				useEmail={v.Login.use_email === true}
				savedUsername={typeof cur(`${setting.key}.username`) === 'string'
					? (cur(`${setting.key}.username`) as string)
					: ''}
				loggedIn={cur(setting.key) === 'logged_in'}
				onSubmit={(username, password) => onSubmitLogin!(setting.key, username, password)}
			/>
		{:else if onSubmitWebLogin && onLogoutLogin && v.Login.method === 'web'}
			<WebLoginSetting
				title={setting.title}
				url={v.Login.url ?? null}
				logoutTitle={v.Login.logout_title ?? null}
				localStorageKeys={v.Login.local_storage_keys ?? []}
				clearCookiesOnLogOut={v.Login.clear_cookies_on_log_out === true}
				loggedIn={typeof cur(setting.key) === 'string' && cur(setting.key) !== ''}
				{sourceId}
				onSubmit={(cookies, ls) => onSubmitWebLogin!(setting.key, cookies, ls)}
				onLogout={(clearCookies) => onLogoutLogin!(setting.key, clearCookies)}
				{browserLogin}
				onStartBrowserLogin={onStartBrowserLogin
					? () => onStartBrowserLogin!(setting.key)
					: undefined}
				onCompleteBrowserLogin={onCompleteBrowserLogin
					? (session) => onCompleteBrowserLogin!(setting.key, session)
					: undefined}
				onCancelBrowserLogin={onCancelBrowserLogin
					? (session) => onCancelBrowserLogin!(setting.key, session)
					: undefined}
			/>
		{:else if onStartOAuth && onCompleteOAuth && onLogoutLogin && v.Login.method === 'oauth'}
			<OAuthSetting
				title={setting.title}
				logoutTitle={v.Login.logout_title ?? null}
				clearCookiesOnLogOut={v.Login.clear_cookies_on_log_out === true}
				loggedIn={typeof cur(setting.key) === 'string' && cur(setting.key) !== ''}
				onStart={() => onStartOAuth!(setting.key)}
				onComplete={(url) => onCompleteOAuth!(setting.key, url)}
				onLogout={(clearCookies) => onLogoutLogin!(setting.key, clearCookies)}
			/>
		{:else}
			<div class="setting-row setting-row--disabled">
				<span class="setting-row__title">{setting.title}</span>
				<span class="faint">
					{v.Login.method === 'basic'
						? $_('settings.loginClientOnly')
						: $_('settings.oauthUnsupported')}
				</span>
			</div>
		{/if}
	{:else if v === 'Button' || (typeof v === 'object' && 'Button' in v)}
		<ButtonRow
			title={setting.title}
			hasAction={!!setting.notification && !!onPressButton}
			onPress={() => onPressButton?.(setting) ?? Promise.resolve()}
		/>
	{:else if typeof v === 'object' && 'Group' in v}
		<div class="setting-group">
			<div class="setting-group__title">{setting.title}</div>
			{#each v.Group.items as item (item.key)}
				<SettingItem
					setting={item}
					{sourceId}
					{values}
					{edits}
					{onEdit}
					{onPressButton}
					{onSubmitLogin}
					{onSubmitWebLogin}
					{onLogoutLogin}
					{browserLogin}
					{onStartBrowserLogin}
					{onCompleteBrowserLogin}
					{onCancelBrowserLogin}
					{onStartOAuth}
					{onCompleteOAuth}
				/>
			{/each}
			{#if v.Group.footer}<div class="faint setting-group__footer">{v.Group.footer}</div>{/if}
		</div>
	{:else if typeof v === 'object' && 'Page' in v}
		{@const pageIcon = v.Page.icon}
		<div class="setting-group">
			<div class="setting-group__title">
				{#if pageIcon?.type === 'url'}
					<img class="setting-group__icon" src={pageIcon.url} alt="" />
				{/if}
				{setting.title}
			</div>
			{#if v.Page.info}<div class="faint setting-group__info">{v.Page.info}</div>{/if}
			{#each v.Page.items as item (item.key)}
				<SettingItem
					setting={item}
					{sourceId}
					{values}
					{edits}
					{onEdit}
					{onPressButton}
					{onSubmitLogin}
					{onSubmitWebLogin}
					{onLogoutLogin}
					{browserLogin}
					{onStartBrowserLogin}
					{onCompleteBrowserLogin}
					{onCancelBrowserLogin}
					{onStartOAuth}
					{onCompleteOAuth}
				/>
			{/each}
		</div>
	{:else if typeof v === 'object' && 'Toggle' in v}
		<label class="setting-row">
			<span class="setting-row__title">
				{setting.title}
				{#if v.Toggle.subtitle}<div class="faint setting-row__sub">{v.Toggle.subtitle}</div>{/if}
			</span>
			<Switch
				checked={(cur(setting.key) ?? v.Toggle.default) === true}
				onChange={(on) => onEdit(setting.key, on)}
				label={setting.title}
			/>
		</label>
	{:else if typeof v === 'object' && 'Select' in v}
		<label class="setting-row setting-row--col">
			<span class="setting-row__title">{setting.title}</span>
			<Select
				value={(cur(setting.key) as string) ?? v.Select.default ?? ''}
				onchange={(e) => {
					const val = e.currentTarget.value;
					onEdit(setting.key, val);
				}}
			>
				{#each v.Select.values as opt, i (opt)}
					<option value={opt}>{v.Select.titles?.[i] ?? opt}</option>
				{/each}
			</Select>
		</label>
	{:else if typeof v === 'object' && 'MultiSelect' in v}
		{@const selected = strArr(cur(setting.key) ?? v.MultiSelect.default ?? [])}
		<div class="setting-row">
			<span class="setting-row__title">{setting.title}</span>
			<Button size="sm" onclick={() => (msOpen = true)}>
				{$_('settings.selectedCount', { values: { n: selected.length } })}
			</Button>
			<Modal open={msOpen} title={setting.title} onClose={() => (msOpen = false)}>
				<div class="setting-ms">
					{#each v.MultiSelect.values as opt, i (opt)}
						{@const on = selected.includes(opt)}
						<label class="setting-ms__row">
							<span>{v.MultiSelect.titles?.[i] ?? opt}</span>
							<Switch
								checked={on}
								onChange={() =>
									onEdit(setting.key, on ? selected.filter((x) => x !== opt) : [...selected, opt])}
								label={v.MultiSelect.titles?.[i] ?? opt}
							/>
						</label>
					{/each}
				</div>
			</Modal>
		</div>
	{:else if typeof v === 'object' && 'Text' in v}
		<label class="setting-row setting-row--col">
			<span class="setting-row__title">{setting.title}</span>
			<Input
				type={v.Text.secure ? 'password' : 'text'}
				placeholder={v.Text.placeholder ?? ''}
				value={(cur(setting.key) as string) ?? v.Text.default ?? ''}
				oninput={(e) => {
					const val = e.currentTarget.value;
					onEdit(setting.key, val);
				}}
			/>
		</label>
	{:else if typeof v === 'object' && 'Stepper' in v}
		<div class="setting-row">
			<span class="setting-row__title">{setting.title}</span>
			<StepperInput
				value={num(cur(setting.key) ?? v.Stepper.default, v.Stepper.minimum_value)}
				min={v.Stepper.minimum_value}
				max={v.Stepper.maximum_value}
				step={v.Stepper.step_value ?? 1}
				onCommit={(n) => onEdit(setting.key, n)}
			/>
		</div>
	{:else if typeof v === 'object' && 'Segment' in v}
		<div class="setting-row setting-row--col">
			<span class="setting-row__title">{setting.title}</span>
			<SegmentedControl
				options={v.Segment.options.map((opt, i) => ({ value: i, label: opt }))}
				value={num(cur(setting.key) ?? v.Segment.default, 0)}
				onChange={(i) => onEdit(setting.key, i)}
			/>
		</div>
	{:else if typeof v === 'object' && 'Link' in v}
		<a
			class="setting-link"
			href={v.Link.url}
			target={v.Link.external === true ? '_blank' : undefined}
			rel={v.Link.external === true ? 'noreferrer' : undefined}
		>
			{setting.title}
			{#if v.Link.external === true}<ExternalLink size={13} />{/if}
		</a>
	{:else}
		<div class="setting-row faint">
			{setting.title}{$_('settings.unsupportedType', {
				values: {
					kind:
						typeof v === 'string' ? v : v && typeof v === 'object' ? Object.keys(v)[0] : String(v)
				}
			})}
		</div>
	{/if}

	{#if setting.notification}
		<div class="faint setting-notification">{setting.notification}</div>
	{/if}
{/if}
