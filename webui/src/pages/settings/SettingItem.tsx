import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { Trans, useTranslation } from 'react-i18next';
import { ExternalLink } from 'lucide-react';
import type { SourceSetting } from '../../api/types';
import { Button, Chip, Input, Modal, SegmentedControl, Select, Switch, pushToast } from '../../components';

export interface SettingItemProps {
	setting: SourceSetting;
	/** Values already saved on the server */
	values: Record<string, unknown>;
	/** Local unsaved edits, shown in preference to values */
	edits: Record<string, unknown>;
	onEdit: (key: string, value: unknown) => void;
	/** Button setting pressed: send its notification to the source */
	onPressButton?: (setting: SourceSetting) => Promise<void>;
	/** Basic login: the outer layer provides the endpoint call; when absent the login item degrades to a read-only hint */
	onSubmitLogin?: (key: string, username: string, password: string) => Promise<void>;
	/** Web login: paste cookies in exchange for a logged-in state */
	onSubmitWebLogin?: (key: string, cookies: Record<string, string>, localStorage: Record<string, string>) => Promise<void>;
	/** Source-level logout: when clearCookies is true the login cookies are cleared too */
	onLogoutLogin?: (key: string, clearCookies: boolean) => Promise<void>;
	/** OAuth login: get the authorization URL (opened in a new tab), completed by pasting the callback string */
	onStartOAuth?: (key: string) => Promise<{ url: string }>;
	onCompleteOAuth?: (key: string, url: string) => Promise<void>;
}

function strArr(v: unknown): string[] {
	return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

function num(v: unknown, fallback: number): number {
	return typeof v === 'number' && Number.isFinite(v) ? v : fallback;
}

/** The number input allows intermediate states (cleared, a lone minus sign); the value clamps back into min/max only on blur */
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

/** Basic login form: when logged in, prefills the username and marks the state; the source validates on submit, and success writes back the credentials */
function LoginSetting({
	title,
	useEmail,
	savedUsername,
	loggedIn,
	onSubmit
}: {
	title: string;
	useEmail: boolean;
	/** Username already stored on the server, prefilled into the input */
	savedUsername: string;
	/** Already marked logged-in on the server */
	loggedIn: boolean;
	onSubmit: (username: string, password: string) => Promise<void>;
}) {
	const { t } = useTranslation();
	const [username, setUsername] = useState(savedUsername);
	const [password, setPassword] = useState('');
	const [busy, setBusy] = useState(false);
	return (
		<div className="setting-row setting-row--col">
			<div className="setting-login__head">
				<span className="setting-row__title">{title}</span>
				{loggedIn && <Chip color="success">{t('settingItem.loggedIn')}</Chip>}
			</div>
			<Input
				placeholder={useEmail ? t('settingItem.email') : t('settingItem.username')}
				autoComplete={useEmail ? 'email' : 'username'}
				value={username}
				onChange={(e) => setUsername(e.currentTarget.value)}
			/>
			<Input
				type="password"
				placeholder={t('settingItem.password')}
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
				{loggedIn ? t('settingItem.relogin') : t('settingItem.login')}
			</Button>
		</div>
	);
}

/**
 * Parse pasted cookies: accepts a `Cookie:` request header (name=value; …),
 * a `{name: value}` JSON object, or a browser-exported [{name, value, …}] array
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

/** The logged-in row: title + logged-in chip + logout button with confirmation (shared by web/oauth login) */
function LoginLogoutRow({
	title,
	logoutTitle,
	clearCookiesOnLogOut,
	onLogout
}: {
	title: string;
	logoutTitle: string | null;
	clearCookiesOnLogOut: boolean;
	onLogout: (clearCookies: boolean) => Promise<void>;
}) {
	const { t } = useTranslation();
	const [busy, setBusy] = useState(false);
	const [confirmOpen, setConfirmOpen] = useState(false);
	const [clearCookies, setClearCookies] = useState(clearCookiesOnLogOut);
	return (
		<div className="setting-row setting-row--col">
			<div className="setting-login__head">
				<span className="setting-row__title">{title}</span>
				<Chip color="success">{t('settingItem.loggedIn')}</Chip>
			</div>
			<Button
				size="sm"
				onClick={() => {
					setClearCookies(clearCookiesOnLogOut);
					setConfirmOpen(true);
				}}
			>
				{logoutTitle ?? t('settingItem.logout')}
			</Button>
			<Modal
				open={confirmOpen}
				title={logoutTitle ?? t('settingItem.logout')}
				onClose={() => setConfirmOpen(false)}
				footer={
					<>
						<Button onClick={() => setConfirmOpen(false)}>{t('common.cancel')}</Button>
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
							{logoutTitle ?? t('settingItem.logout')}
						</Button>
					</>
				}
			>
				<p>{t('settingItem.logoutConfirm')}</p>
				<label className="setting-row">
					<span className="setting-row__title">{t('settingItem.clearCookies')}</span>
					<Switch checked={clearCookies} onChange={setClearCookies} label={t('settingItem.clearCookies')} />
				</label>
			</Modal>
		</div>
	);
}

/** Web login: the server has no webview, so the user logs in elsewhere and pastes that host's cookies in exchange for a logged-in state */
function WebLoginSetting({
	title,
	url,
	logoutTitle,
	localStorageKeys,
	clearCookiesOnLogOut,
	loggedIn,
	onSubmit,
	onLogout
}: {
	title: string;
	/** Login page address (the setting's declared url), telling the user where to log in */
	url: string | null;
	logoutTitle: string | null;
	localStorageKeys: string[];
	clearCookiesOnLogOut: boolean;
	loggedIn: boolean;
	onSubmit: (cookies: Record<string, string>, localStorage: Record<string, string>) => Promise<void>;
	onLogout: (clearCookies: boolean) => Promise<void>;
}) {
	const { t } = useTranslation();
	const [text, setText] = useState('');
	const [lsValues, setLsValues] = useState<Record<string, string>>({});
	const [busy, setBusy] = useState(false);

	if (loggedIn) {
		return (
			<LoginLogoutRow
				title={title}
				logoutTitle={logoutTitle}
				clearCookiesOnLogOut={clearCookiesOnLogOut}
				onLogout={onLogout}
			/>
		);
	}

	return (
		<div className="setting-row setting-row--col">
			<div className="setting-login__head">
				<span className="setting-row__title">{title}</span>
			</div>
			<span className="faint">
				{url ? (
					<Trans
						i18nKey="settingItem.webLoginHintWithUrl"
						values={{ url }}
						components={{ 1: <a href={url} target="_blank" rel="noreferrer" /> }}
					/>
				) : (
					t('settingItem.webLoginHint')
				)}
			</span>
			<textarea
				className="setting-textarea"
				rows={3}
				placeholder={t('settingItem.cookiePlaceholder')}
				value={text}
				onChange={(e) => setText(e.currentTarget.value)}
			/>
			{localStorageKeys.map((lsKey) => (
				<Input
					key={lsKey}
					placeholder={t('settingItem.localStoragePlaceholder', { key: lsKey })}
					value={lsValues[lsKey] ?? ''}
					onChange={(e) => {
						// currentTarget is already null by the time the deferred updater runs, so take the value first
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
						pushToast(t('settingItem.cookieParseError'), 'error');
						return;
					}
					setBusy(true);
					void onSubmit(cookies, lsValues).finally(() => setBusy(false));
				}}
			>
				{t('settingItem.login')}
			</Button>
		</div>
	);
}

/**
 * OAuth login: the authorization page opens in a new tab (URL built by the server, with a challenge when
 * using pkce); the browser cannot intercept the custom-scheme redirect, so after authorizing the user
 * manually pastes the callback URL from the address bar to complete login
 */
function OAuthSetting({
	title,
	logoutTitle,
	clearCookiesOnLogOut,
	loggedIn,
	onStart,
	onComplete,
	onLogout
}: {
	title: string;
	logoutTitle: string | null;
	clearCookiesOnLogOut: boolean;
	loggedIn: boolean;
	/** Get the authorization URL (built by the server) and open it in a new tab */
	onStart: () => Promise<{ url: string }>;
	onComplete: (url: string) => Promise<void>;
	onLogout: (clearCookies: boolean) => Promise<void>;
}) {
	const { t } = useTranslation();
	const [text, setText] = useState('');
	const [busy, setBusy] = useState(false);
	const [startBusy, setStartBusy] = useState(false);

	if (loggedIn) {
		return (
			<LoginLogoutRow
				title={title}
				logoutTitle={logoutTitle}
				clearCookiesOnLogOut={clearCookiesOnLogOut}
				onLogout={onLogout}
			/>
		);
	}

	return (
		<div className="setting-row setting-row--col">
			<div className="setting-login__head">
				<span className="setting-row__title">{title}</span>
			</div>
			<span className="faint">{t('settingItem.oauthHint')}</span>
			<Button
				size="sm"
				loading={startBusy}
				onClick={() => {
					// window.open after an await is popup-blocked (Safari always blocks): open a blank tab synchronously
					// and navigate it once the authorization URL arrives; if even the synchronous open is blocked, fall back to opening directly
					const tab = window.open('', '_blank');
					setStartBusy(true);
					void onStart()
						.then((data) => {
							if (tab && !tab.closed) {
								tab.location.href = data.url;
							} else {
								window.open(data.url, '_blank', 'noreferrer');
							}
						})
						.catch(() => tab?.close())
						.finally(() => setStartBusy(false));
				}}
			>
				{t('settingItem.login')}
			</Button>
			<textarea
				className="setting-textarea"
				rows={2}
				placeholder={t('settingItem.oauthPlaceholder')}
				value={text}
				onChange={(e) => setText(e.currentTarget.value)}
			/>
			<Button
				size="sm"
				loading={busy}
				disabled={!text.trim()}
				onClick={() => {
					setBusy(true);
					void onComplete(text.trim()).finally(() => setBusy(false));
				}}
			>
				{t('settingItem.done')}
			</Button>
		</div>
	);
}

/** Button setting: pressing sends its notification to the source (Aidoku semantics); with no action string declared, pressing is a no-op, so disable it outright */
function ButtonRow({
	title,
	hasAction,
	onPress
}: {
	title: string;
	hasAction: boolean;
	onPress: () => Promise<void>;
}) {
	const { t } = useTranslation();
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
			{!hasAction && <span className="faint">{t('settingItem.buttonNoAction')}</span>}
		</div>
	);
}

export function SettingItem({
	setting,
	values,
	edits,
	onEdit,
	onPressButton,
	onSubmitLogin,
	onSubmitWebLogin,
	onLogoutLogin,
	onStartOAuth,
	onCompleteOAuth
}: SettingItemProps) {
	const { t } = useTranslation();
	const [msOpen, setMsOpen] = useState(false);

	// The current value prefers unsaved edits, then the server-stored value
	const cur = (key: string): unknown => (key in edits ? edits[key] : values[key]);

	// requires / requires_false control visibility, evaluated live against the current values
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
	} else if (typeof v === 'object' && 'Picker' in v) {
		const p = v.Picker;
		const opts = p.values;
		const selected = typeof cur(setting.key) === 'string' ? (cur(setting.key) as string) : (p.default ?? '');
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
							{p.titles?.[i] ?? opt}
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
					title={setting.title ?? ''}
					useEmail={v.Login.use_email === true}
					savedUsername={typeof savedUser === 'string' ? savedUser : ''}
					loggedIn={marker === 'logged_in'}
					onSubmit={(username, password) => onSubmitLogin(setting.key, username, password)}
				/>
			);
		} else if (onSubmitWebLogin && onLogoutLogin && method === 'web') {
			// Aidoku judges the logged-in state by a non-empty marker (not limited to the literal "logged_in")
			body = (
				<WebLoginSetting
					title={setting.title ?? ''}
					url={v.Login.url ?? null}
					logoutTitle={v.Login.logout_title ?? null}
					localStorageKeys={v.Login.local_storage_keys ?? []}
					clearCookiesOnLogOut={v.Login.clear_cookies_on_log_out === true}
					loggedIn={typeof marker === 'string' && marker !== ''}
					onSubmit={(cookies, ls) => onSubmitWebLogin(setting.key, cookies, ls)}
					onLogout={(clearCookies) => onLogoutLogin(setting.key, clearCookies)}
				/>
			);
		} else if (onStartOAuth && onCompleteOAuth && onLogoutLogin && method === 'oauth') {
			// Logged-in check same as web: a non-empty marker (the <key> stores a token or callback string) means logged in
			body = (
				<OAuthSetting
					title={setting.title ?? ''}
					logoutTitle={v.Login.logout_title ?? null}
					clearCookiesOnLogOut={v.Login.clear_cookies_on_log_out === true}
					loggedIn={typeof marker === 'string' && marker !== ''}
					onStart={() => onStartOAuth(setting.key)}
					onComplete={(url) => onCompleteOAuth(setting.key, url)}
					onLogout={(clearCookies) => onLogoutLogin(setting.key, clearCookies)}
				/>
			);
		} else {
			body = (
				<div className="setting-row setting-row--disabled">
					<span className="setting-row__title">{setting.title}</span>
					<span className="faint">{method === 'basic' ? t('settingItem.basicClientOnly') : t('settingItem.oauthUnsupported')}</span>
				</div>
			);
		}
	} else if (v === 'Button' || (typeof v === 'object' && 'Button' in v)) {
		body = (
			<ButtonRow
				title={setting.title ?? ''}
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
						values={values}
						edits={edits}
						onEdit={onEdit}
						onPressButton={onPressButton}
						onSubmitLogin={onSubmitLogin}
						onSubmitWebLogin={onSubmitWebLogin}
						onLogoutLogin={onLogoutLogin}
						onStartOAuth={onStartOAuth}
						onCompleteOAuth={onCompleteOAuth}
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
						values={values}
						edits={edits}
						onEdit={onEdit}
						onPressButton={onPressButton}
						onSubmitLogin={onSubmitLogin}
						onSubmitWebLogin={onSubmitWebLogin}
						onLogoutLogin={onLogoutLogin}
						onStartOAuth={onStartOAuth}
						onCompleteOAuth={onCompleteOAuth}
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
					label={setting.title ?? ''}
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
					{t('settingItem.selectedCount', { count: selected.length })}
				</Button>
				<Modal open={msOpen} title={setting.title ?? ''} onClose={() => setMsOpen(false)}>
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
		// Aidoku: external goes to the system browser, otherwise an embedded SafariView; on the web this maps to new tab vs current tab
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
		body = <div className="setting-row faint">{t('settingItem.unsupportedType', { title: setting.title, kind })}</div>;
	}

	return (
		<>
			{body}
			{setting.notification && <div className="faint setting-notification">{setting.notification}</div>}
		</>
	);
}
