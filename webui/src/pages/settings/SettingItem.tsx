import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { ExternalLink } from 'lucide-react';
import type { Setting } from '../../api/types';
import { Button, Chip, Input, Modal, SegmentedControl, Select, Switch } from '../../components';

export interface SettingItemProps {
	setting: Setting;
	/** 服务端已保存的值 */
	values: Record<string, unknown>;
	/** 本地未保存的改动，优先于 values 显示 */
	edits: Record<string, unknown>;
	onEdit: (key: string, value: unknown) => void;
	/** 基本登录：由外层提供端点调用，缺省时 login 项降级为只读提示 */
	onSubmitLogin?: (key: string, username: string, password: string) => Promise<void>;
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

export function SettingItem({ setting, values, edits, onEdit, onSubmitLogin }: SettingItemProps) {
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
		body =
			onSubmitLogin && method === 'basic' ? (
				<LoginSetting
					title={setting.title}
					useEmail={v.Login.use_email === true}
					savedUsername={typeof savedUser === 'string' ? savedUser : ''}
					loggedIn={cur(setting.key) === 'logged_in'}
					onSubmit={(username, password) => onSubmitLogin(setting.key, username, password)}
				/>
			) : (
				<div className="setting-row setting-row--disabled">
					<span className="setting-row__title">{setting.title}</span>
					<span className="faint">
						{method === 'basic' ? '该操作需要在客户端完成' : '暂不支持 OAuth / 网页登录'}
					</span>
				</div>
			);
	} else if (v === 'Button' || (typeof v === 'object' && 'Button' in v)) {
		body = (
			<div className="setting-row setting-row--disabled">
				<span className="setting-row__title">{setting.title}</span>
				<span className="faint">该操作需要在客户端完成</span>
			</div>
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
						onSubmitLogin={onSubmitLogin}
					/>
				))}
				{v.Group.footer && <div className="faint setting-group__footer">{v.Group.footer}</div>}
			</div>
		);
	} else if (typeof v === 'object' && 'Page' in v) {
		body = (
			<div className="setting-group">
				<div className="setting-group__title">{setting.title}</div>
				{v.Page.items.map((item) => (
					<SettingItem
						key={item.key}
						setting={item}
						values={values}
						edits={edits}
						onEdit={onEdit}
						onSubmitLogin={onSubmitLogin}
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
		body = (
			<a className="setting-link" href={v.Link.url} target="_blank" rel="noreferrer">
				{setting.title}
				<ExternalLink size={13} />
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
