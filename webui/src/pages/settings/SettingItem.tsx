import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { ExternalLink } from 'lucide-react';
import type { BrowserLoginSession, Setting } from '../../api/types';
import { Button, Chip, Input, Modal, SegmentedControl, Select, Switch, pushToast } from '../../components';
import { BrowserLoginViewer } from './BrowserLoginViewer';

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
	onSubmitWebLogin?: (key: string, cookies: Record<string, string>, localStorage: Record<string, string>) => Promise<void>;
	/** 源级登出：clearCookies 为真时连登录 cookie 一起清 */
	onLogoutLogin?: (key: string, clearCookies: boolean) => Promise<void>;
	/** 服务端配了 sidecar（capabilities.browserLogin）时 web 登录项显示交互登录入口 */
	browserLogin?: boolean;
	/** sidecar 交互登录：开会话返回句柄与登录页地址 */
	onStartBrowserLogin?: (key: string) => Promise<BrowserLoginSession>;
	/** sidecar 交互登录：完成（收割 cookie）与取消（清理 context） */
	onCompleteBrowserLogin?: (key: string, session: string) => Promise<void>;
	onCancelBrowserLogin?: (key: string, session: string) => Promise<void>;
}

function strArr(v: unknown): string[] {
	return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

function num(v: unknown, fallback: number): number {
	return typeof v === 'number' && Number.isFinite(v) ? v : fallback;
}

/** number 输入允许中间态（清空、负号），失焦才夹回 min/max */
function StepperInput({
	value,
	min,
	max,
	step,
	onCommit
}: {
	value: number;
	min: number;
	max: number;
	step: number;
	onCommit: (n: number) => void;
}) {
	const [text, setText] = useState(String(value));
	useEffect(() => setText(String(value)), [value]);
	return (
		<Input
			className="setting-stepper__input"
			type="number"
			min={min}
			max={max}
			step={step}
			value={text}
			onChange={(e) => {
				const raw = e.currentTarget.value;
				setText(raw);
				const n = Number(raw);
				if (raw !== '' && Number.isFinite(n)) onCommit(n);
			}}
			onBlur={() => {
				const n = Number(text);
				const clamped = text !== '' && Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : value;
				setText(String(clamped));
				if (clamped !== value) onCommit(clamped);
			}}
		/>
	);
}

/** 基本登录表单：已登录时回填用户名并标记登录态，提交后由源校验，成功即写回凭据 */
function LoginSetting({
	title,
	useEmail,
	savedUsername,
	loggedIn,
	onSubmit
}: {
	title: string;
	useEmail: boolean;
	/** 服务端已存的用户名，回填输入框 */
	savedUsername: string;
	/** 服务端已标记登录 */
	loggedIn: boolean;
	onSubmit: (username: string, password: string) => Promise<void>;
}) {
	const [username, setUsername] = useState(savedUsername);
	const [password, setPassword] = useState('');
	const [busy, setBusy] = useState(false);
	return (
		<div className="setting-row setting-row--col">
			<div className="setting-login__head">
				<span className="setting-row__title">{title}</span>
				{loggedIn && <Chip color="success">已登录</Chip>}
			</div>
			<Input
				placeholder={useEmail ? '邮箱' : '用户名'}
				autoComplete={useEmail ? 'email' : 'username'}
				value={username}
				onChange={(e) => setUsername(e.currentTarget.value)}
			/>
			<Input
				type="password"
				placeholder="密码"
				autoComplete="current-password"
				value={password}
				onChange={(e) => setPassword(e.currentTarget.value)}
			/>
			<Button
				size="sm"
				loading={busy}
				disabled={!username || !password}
				onClick={() => {
					setBusy(true);
					void onSubmit(username, password).finally(() => setBusy(false));
				}}
			>
				{loggedIn ? '重新登录' : '登录'}
			</Button>
		</div>
	);
}

/**
 * 解析粘贴的 cookie：接受 `Cookie:` 请求头（name=value; …）、
 * `{name: value}` JSON 对象，或浏览器导出的 [{name, value, …}] 数组
 */
function parseCookies(text: string): Record<string, string> | null {
	const trimmed = text.trim().replace(/^cookie:\s*/i, '');
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

/** 网页登录：服务端没有 webview，用户登录后粘贴该 host 的 cookie 换取登录态；
 *  服务端配了 sidecar 时也可以开交互会话，在实时视图里完成登录 */
function WebLoginSetting({
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
}: {
	title: string;
	/** 登录页地址（设置声明的 url），提示用户去哪里登录 */
	url: string | null;
	logoutTitle: string | null;
	localStorageKeys: string[];
	clearCookiesOnLogOut: boolean;
	loggedIn: boolean;
	sourceId: number;
	onSubmit: (cookies: Record<string, string>, localStorage: Record<string, string>) => Promise<void>;
	onLogout: (clearCookies: boolean) => Promise<void>;
	browserLogin?: boolean;
	onStartBrowserLogin?: () => Promise<BrowserLoginSession>;
	onCompleteBrowserLogin?: (session: string) => Promise<void>;
	onCancelBrowserLogin?: (session: string) => Promise<void>;
}) {
	const [text, setText] = useState('');
	const [lsValues, setLsValues] = useState<Record<string, string>>({});
	const [busy, setBusy] = useState(false);
	const [confirmOpen, setConfirmOpen] = useState(false);
	const [clearCookies, setClearCookies] = useState(clearCookiesOnLogOut);
	const [browserSession, setBrowserSession] = useState<BrowserLoginSession | null>(null);
	const [browserBusy, setBrowserBusy] = useState(false);

	const canBrowserLogin = browserLogin && onStartBrowserLogin && onCompleteBrowserLogin && onCancelBrowserLogin;

	async function closeBrowserSession(cancel: boolean) {
		const session = browserSession;
		setBrowserSession(null);
		if (session && cancel && onCancelBrowserLogin) {
			await onCancelBrowserLogin(session.session).catch(() => {});
		}
	}

	if (loggedIn) {
		return (
			<div className="setting-row setting-row--col">
				<div className="setting-login__head">
					<span className="setting-row__title">{title}</span>
					<Chip color="success">已登录</Chip>
				</div>
				<Button
					size="sm"
					onClick={() => {
						setClearCookies(clearCookiesOnLogOut);
						setConfirmOpen(true);
					}}
				>
					{logoutTitle ?? '登出'}
				</Button>
				<Modal
					open={confirmOpen}
					title={logoutTitle ?? '登出'}
					onClose={() => setConfirmOpen(false)}
					footer={
						<>
							<Button onClick={() => setConfirmOpen(false)}>取消</Button>
							<Button
								variant="primary"
								loading={busy}
								onClick={() => {
									setBusy(true);
									void onLogout(clearCookies)
										.then(() => setConfirmOpen(false))
										.finally(() => setBusy(false));
								}}
							>
								{logoutTitle ?? '登出'}
							</Button>
						</>
					}
				>
					<p>退出该源的登录？登录态与已保存的 cookie 键值会被清除。</p>
					<label className="setting-row">
						<span className="setting-row__title">同时清除登录 cookie</span>
						<Switch checked={clearCookies} onChange={setClearCookies} label="同时清除登录 cookie" />
					</label>
				</Modal>
			</div>
		);
	}

	return (
		<div className="setting-row setting-row--col">
			<div className="setting-login__head">
				<span className="setting-row__title">{title}</span>
			</div>
			{canBrowserLogin && (
				<>
					<span className="faint">在服务端的浏览器里打开登录页，登录完成后 cookie 自动回收</span>
					<Button
						size="sm"
						loading={browserBusy}
						onClick={() => {
							setBrowserBusy(true);
							void onStartBrowserLogin()
								.then(setBrowserSession)
								.finally(() => setBrowserBusy(false));
						}}
					>
						在浏览器中打开登录页
					</Button>
				</>
			)}
			<span className="faint">
				{url ? (
					<>
						先在浏览器登录{' '}
						<a href={url} target="_blank" rel="noreferrer">
							{url}
						</a>
						，再粘贴该站的 Cookie 请求头或 JSON 导出
					</>
				) : (
					'先在浏览器登录该源，再粘贴登录站的 Cookie 请求头或 JSON 导出'
				)}
			</span>
			<textarea
				className="setting-textarea"
				rows={3}
				placeholder={'name=value; name2=value2\n或 {"name": "value"}'}
				value={text}
				onChange={(e) => setText(e.currentTarget.value)}
			/>
			{localStorageKeys.map((lsKey) => (
				<Input
					key={lsKey}
					placeholder={`localStorage：${lsKey}`}
					value={lsValues[lsKey] ?? ''}
					onChange={(e) => {
						// 更新函数延后执行时 currentTarget 已被置 null，必须先取值
						const v = e.currentTarget.value;
						setLsValues((cur) => ({ ...cur, [lsKey]: v }));
					}}
				/>
			))}
			<Button
				size="sm"
				loading={busy}
				disabled={!text.trim()}
				onClick={() => {
					const cookies = parseCookies(text);
					if (!cookies) {
						pushToast('无法解析 Cookie，支持请求头或 JSON 格式', 'error');
						return;
					}
					setBusy(true);
					void onSubmit(cookies, lsValues).finally(() => setBusy(false));
				}}
			>
				登录
			</Button>
			{browserSession && onCompleteBrowserLogin && (
				<BrowserLoginViewer
					sourceId={sourceId}
					session={browserSession.session}
					loginUrl={browserSession.loginUrl}
					onComplete={async () => {
						await onCompleteBrowserLogin(browserSession.session);
						await closeBrowserSession(false);
					}}
					onCancel={() => void closeBrowserSession(true)}
				/>
			)}
		</div>
	);
}

/** Button 设置：按下把 notification 发给源（Aidoku 语义）；没声明动作串时按下也是空操作，直接禁用 */
function ButtonRow({
	title,
	hasAction,
	onPress
}: {
	title: string;
	hasAction: boolean;
	onPress: () => Promise<void>;
}) {
	const [busy, setBusy] = useState(false);
	return (
		<div className="setting-row">
			<Button
				size="sm"
				loading={busy}
				disabled={!hasAction}
				onClick={() => {
					setBusy(true);
					void onPress().finally(() => setBusy(false));
				}}
			>
				{title}
			</Button>
			{!hasAction && <span className="faint">该按钮未声明动作</span>}
		</div>
	);
}

export function SettingItem({
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
	onCancelBrowserLogin
}: SettingItemProps) {
	const [msOpen, setMsOpen] = useState(false);

	// 当前值优先取未保存的修改，其次取服务器已存值
	const cur = (key: string): unknown => (key in edits ? edits[key] : values[key]);

	// requires / requires_false 控制显隐，按当前值实时判断
	if (setting.requires && cur(setting.requires) !== true) return null;
	if (setting.requires_false && cur(setting.requires_false) === true) return null;

	const v = setting.value;
	let body: ReactNode;

	if (setting.type === 'editable-list') {
		body = (
			<label className="setting-row setting-row--col">
				<span className="setting-row__title">{setting.title}</span>
				<textarea
					className="setting-textarea"
					rows={5}
					value={strArr(cur(setting.key)).join('\n')}
					onChange={(e) => onEdit(setting.key, e.currentTarget.value.split('\n'))}
				/>
			</label>
		);
	} else if (setting.type === 'picker') {
		// types.ts 的 SettingValue 没有 Picker 变体，wire 上与 Select 同构，宽松取值
		const p = (
			v as unknown as Record<
				string,
				{ values?: string[]; titles?: (string | null)[] | null; default?: string | null } | undefined
			>
		).Picker;
		const opts = p?.values ?? [];
		const selected = typeof cur(setting.key) === 'string' ? (cur(setting.key) as string) : (p?.default ?? '');
		body = (
			<div className="setting-row setting-row--col">
				<span className="setting-row__title">{setting.title}</span>
				<div className="setting-chips">
					{opts.map((opt, i) => (
						<Chip
							key={opt}
							color={selected === opt ? 'accent' : 'default'}
							onClick={() => onEdit(setting.key, opt)}
						>
							{p?.titles?.[i] ?? opt}
						</Chip>
					))}
				</div>
			</div>
		);
	} else if (typeof v === 'object' && 'Login' in v) {
		const method = v.Login.method;
		const savedUser = cur(`${setting.key}.username`);
		const marker = cur(setting.key);
		if (onSubmitLogin && method === 'basic') {
			body = (
				<LoginSetting
					title={setting.title}
					useEmail={v.Login.use_email === true}
					savedUsername={typeof savedUser === 'string' ? savedUser : ''}
					loggedIn={marker === 'logged_in'}
					onSubmit={(username, password) => onSubmitLogin(setting.key, username, password)}
				/>
			);
		} else if (onSubmitWebLogin && onLogoutLogin && method === 'web') {
			// Aidoku 以 marker 非空判登录态（不限于 "logged_in" 字面值）
			body = (
				<WebLoginSetting
					title={setting.title}
					url={v.Login.url ?? null}
					logoutTitle={v.Login.logout_title ?? null}
					localStorageKeys={v.Login.local_storage_keys ?? []}
					clearCookiesOnLogOut={v.Login.clear_cookies_on_log_out === true}
					loggedIn={typeof marker === 'string' && marker !== ''}
					sourceId={sourceId}
					onSubmit={(cookies, ls) => onSubmitWebLogin(setting.key, cookies, ls)}
					onLogout={(clearCookies) => onLogoutLogin(setting.key, clearCookies)}
					browserLogin={browserLogin}
					onStartBrowserLogin={onStartBrowserLogin ? () => onStartBrowserLogin(setting.key) : undefined}
					onCompleteBrowserLogin={
						onCompleteBrowserLogin ? (session) => onCompleteBrowserLogin(setting.key, session) : undefined
					}
					onCancelBrowserLogin={
						onCancelBrowserLogin ? (session) => onCancelBrowserLogin(setting.key, session) : undefined
					}
				/>
			);
		} else {
			body = (
				<div className="setting-row setting-row--disabled">
					<span className="setting-row__title">{setting.title}</span>
					<span className="faint">{method === 'basic' ? '该操作需要在客户端完成' : '暂不支持 OAuth 登录'}</span>
				</div>
			);
		}
	} else if (v === 'Button' || (typeof v === 'object' && 'Button' in v)) {
		body = (
			<ButtonRow
				title={setting.title}
				hasAction={!!setting.notification && !!onPressButton}
				onPress={() => onPressButton?.(setting) ?? Promise.resolve()}
			/>
		);
	} else if (typeof v === 'object' && 'Group' in v) {
		body = (
			<div className="setting-group">
				<div className="setting-group__title">{setting.title}</div>
				{v.Group.items.map((item) => (
					<SettingItem
						key={item.key}
						setting={item}
						sourceId={sourceId}
						values={values}
						edits={edits}
						onEdit={onEdit}
						onPressButton={onPressButton}
						onSubmitLogin={onSubmitLogin}
						onSubmitWebLogin={onSubmitWebLogin}
						onLogoutLogin={onLogoutLogin}
						browserLogin={browserLogin}
						onStartBrowserLogin={onStartBrowserLogin}
						onCompleteBrowserLogin={onCompleteBrowserLogin}
						onCancelBrowserLogin={onCancelBrowserLogin}
					/>
				))}
				{v.Group.footer && <div className="faint setting-group__footer">{v.Group.footer}</div>}
			</div>
		);
	} else if (typeof v === 'object' && 'Page' in v) {
		body = (
			<div className="setting-group">
				<div className="setting-group__title">
					{v.Page.icon?.type === 'url' && <img className="setting-group__icon" src={v.Page.icon.url} alt="" />}
					{setting.title}
				</div>
				{v.Page.info && <div className="faint setting-group__info">{v.Page.info}</div>}
				{v.Page.items.map((item) => (
					<SettingItem
						key={item.key}
						setting={item}
						sourceId={sourceId}
						values={values}
						edits={edits}
						onEdit={onEdit}
						onPressButton={onPressButton}
						onSubmitLogin={onSubmitLogin}
						onSubmitWebLogin={onSubmitWebLogin}
						onLogoutLogin={onLogoutLogin}
						browserLogin={browserLogin}
						onStartBrowserLogin={onStartBrowserLogin}
						onCompleteBrowserLogin={onCompleteBrowserLogin}
						onCancelBrowserLogin={onCancelBrowserLogin}
					/>
				))}
			</div>
		);
	} else if (typeof v === 'object' && 'Toggle' in v) {
		body = (
			<label className="setting-row">
				<span className="setting-row__title">
					{setting.title}
					{v.Toggle.subtitle && <div className="faint setting-row__sub">{v.Toggle.subtitle}</div>}
				</span>
				<Switch
					checked={(cur(setting.key) ?? v.Toggle.default) === true}
					onChange={(on) => onEdit(setting.key, on)}
					label={setting.title}
				/>
			</label>
		);
	} else if (typeof v === 'object' && 'Select' in v) {
		body = (
			<label className="setting-row setting-row--col">
				<span className="setting-row__title">{setting.title}</span>
				<Select
					value={(cur(setting.key) as string) ?? v.Select.default ?? ''}
					onChange={(e) => onEdit(setting.key, e.currentTarget.value)}
				>
					{v.Select.values.map((opt, i) => (
						<option key={opt} value={opt}>
							{v.Select.titles?.[i] ?? opt}
						</option>
					))}
				</Select>
			</label>
		);
	} else if (typeof v === 'object' && 'MultiSelect' in v) {
		const selected = strArr(cur(setting.key) ?? v.MultiSelect.default ?? []);
		body = (
			<div className="setting-row">
				<span className="setting-row__title">{setting.title}</span>
				<Button size="sm" onClick={() => setMsOpen(true)}>
					已选 {selected.length} 项
				</Button>
				<Modal open={msOpen} title={setting.title} onClose={() => setMsOpen(false)}>
					<div className="setting-ms">
						{v.MultiSelect.values.map((opt, i) => {
							const on = selected.includes(opt);
							return (
								<label key={opt} className="setting-ms__row">
									<span>{v.MultiSelect.titles?.[i] ?? opt}</span>
									<Switch
										checked={on}
										onChange={() =>
											onEdit(
												setting.key,
												on ? selected.filter((x) => x !== opt) : [...selected, opt]
											)
										}
										label={v.MultiSelect.titles?.[i] ?? opt}
									/>
								</label>
							);
						})}
					</div>
				</Modal>
			</div>
		);
	} else if (typeof v === 'object' && 'Text' in v) {
		body = (
			<label className="setting-row setting-row--col">
				<span className="setting-row__title">{setting.title}</span>
				<Input
					type={v.Text.secure ? 'password' : 'text'}
					placeholder={v.Text.placeholder ?? ''}
					value={(cur(setting.key) as string) ?? v.Text.default ?? ''}
					onChange={(e) => onEdit(setting.key, e.currentTarget.value)}
				/>
			</label>
		);
	} else if (typeof v === 'object' && 'Stepper' in v) {
		const cv = num(cur(setting.key) ?? v.Stepper.default, v.Stepper.minimum_value);
		body = (
			<div className="setting-row">
				<span className="setting-row__title">{setting.title}</span>
				<StepperInput
					value={cv}
					min={v.Stepper.minimum_value}
					max={v.Stepper.maximum_value}
					step={v.Stepper.step_value ?? 1}
					onCommit={(n) => onEdit(setting.key, n)}
				/>
			</div>
		);
	} else if (typeof v === 'object' && 'Segment' in v) {
		const cv = num(cur(setting.key) ?? v.Segment.default, 0);
		body = (
			<div className="setting-row setting-row--col">
				<span className="setting-row__title">{setting.title}</span>
				<SegmentedControl
					options={v.Segment.options.map((opt, i) => ({ value: i, label: opt }))}
					value={cv}
					onChange={(i) => onEdit(setting.key, i)}
				/>
			</div>
		);
	} else if (typeof v === 'object' && 'Link' in v) {
		// Aidoku：external 走系统浏览器，否则内嵌 SafariView；web 上映射为新标签页 vs 当前标签
		const external = v.Link.external === true;
		body = (
			<a
				className="setting-link"
				href={v.Link.url}
				{...(external ? { target: '_blank', rel: 'noreferrer' } : {})}
			>
				{setting.title}
				{external && <ExternalLink size={13} />}
			</a>
		);
	} else {
		const kind = typeof v === 'string' ? v : v && typeof v === 'object' ? Object.keys(v)[0] : String(v);
		body = <div className="setting-row faint">{setting.title}（暂不支持的类型：{kind}）</div>;
	}

	return (
		<>
			{body}
			{setting.notification && <div className="faint setting-notification">{setting.notification}</div>}
		</>
	);
}
