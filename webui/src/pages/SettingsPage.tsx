import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import {
	ArrowDown,
	ArrowUp,
	Download,
	ExternalLink,
	Pencil,
	Trash2,
	Upload
} from 'lucide-react';
import { ApiError, api, backupUrl, clearToken } from '../api/client';
import {
	useCategories,
	useCreateCategory,
	useDeleteCategory,
	usePatchCategory,
	useRenameCategory,
	useReorderCategories,
	useRestoreBackup,
	useSaveServerSettings,
	useServerSettings
} from '../api/hooks';
import type { BackupRestoreResult, Category } from '../api/types';
import { useTheme } from '../lib/theme';
import type { Theme } from '../lib/theme';
import {
	Badge,
	Button,
	IconButton,
	Input,
	Modal,
	PageHeader,
	SegmentedControl,
	Select,
	Skeleton,
	pushToast
} from '../components';
import './settings.css';

function Section({
	title,
	desc,
	children
}: {
	title: string;
	desc?: string;
	children: ReactNode;
}) {
	return (
		<section className="settings-section card">
			<h2 className="settings-section__title">{title}</h2>
			{desc && <p className="settings-section__desc">{desc}</p>}
			<div className="settings-section__body">{children}</div>
		</section>
	);
}

const READER_MODE_KEY = 'sumidoku.reader.mode';
const READER_DIR_KEY = 'sumidoku.reader.dir';

function stored(key: string, fallback: string): string {
	try {
		return localStorage.getItem(key) ?? fallback;
	} catch {
		return fallback;
	}
}

function AppearanceSection() {
	const [theme, setTheme] = useTheme();
	const [mode, setMode] = useState(() => stored(READER_MODE_KEY, 'paged'));
	const [dir, setDir] = useState(() => stored(READER_DIR_KEY, 'ltr'));

	function persist(key: string, value: string, apply: (v: string) => void) {
		apply(value);
		try {
			localStorage.setItem(key, value);
		} catch {
			// 隐私模式写不进去也照常切换，只是不持久化
		}
	}

	return (
		<Section title="外观" desc="主题和默认阅读方式，仅保存在当前浏览器">
			<div className="settings-row">
				<span className="settings-row__label">主题</span>
				<SegmentedControl<Theme>
					options={[
						{ value: 'system', label: '跟随系统' },
						{ value: 'dark', label: '暗色' },
						{ value: 'light', label: '亮色' }
					]}
					value={theme}
					onChange={setTheme}
				/>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">默认阅读模式</span>
				<Select
					value={mode}
					onChange={(e) => persist(READER_MODE_KEY, e.currentTarget.value, setMode)}
				>
					<option value="paged">翻页</option>
					<option value="vertical">滚动</option>
					<option value="webtoon">条漫</option>
				</Select>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">默认阅读方向</span>
				<Select value={dir} onChange={(e) => persist(READER_DIR_KEY, e.currentTarget.value, setDir)}>
					<option value="ltr">从左到右</option>
					<option value="rtl">从右到左</option>
				</Select>
			</div>
		</Section>
	);
}

function SecuritySection() {
	const navigate = useNavigate();
	const [enabled, setEnabled] = useState<boolean | null>(null);
	const [current, setCurrent] = useState('');
	const [next, setNext] = useState('');
	const [pending, setPending] = useState(false);

	useEffect(() => {
		api<{ enabled: boolean }>('/auth/status', { silent: true })
			.then((s) => setEnabled(s.enabled))
			.catch(() => setEnabled(null));
	}, []);

	async function submit() {
		if (!current || !next || pending) return;
		setPending(true);
		try {
			// 改密码会吊销全部会话 token，成功后必须重新登录
			await api('/auth/password', {
				method: 'PUT',
				body: { currentPassword: current, newPassword: next },
				silent: true
			});
			pushToast('密码已修改，请重新登录', 'info');
			clearToken();
			navigate('/login');
		} catch (e) {
			pushToast(e instanceof ApiError && e.status === 401 ? '当前密码错误' : '修改失败');
		} finally {
			setPending(false);
		}
	}

	async function logout() {
		await api('/auth/logout', { method: 'POST', silent: true }).catch(() => {});
		clearToken();
		navigate('/login');
	}

	return (
		<Section title="安全" desc="web 界面、REST API 与 OPDS 共用同一密码；API key 走服务端配置">
			{enabled === null ? (
				<Skeleton height={48} />
			) : !enabled ? (
				<p className="faint settings-security__off">
					未启用认证，配置 SUMIDOKU_AUTH_PASSWORD 后重启生效
				</p>
			) : (
				<>
					<div className="settings-row">
						<span className="settings-row__label">当前密码</span>
						<Input
							type="password"
							value={current}
							onChange={(e) => setCurrent(e.currentTarget.value)}
						/>
					</div>
					<div className="settings-row">
						<span className="settings-row__label">新密码</span>
						<Input
							type="password"
							value={next}
							onChange={(e) => setNext(e.currentTarget.value)}
						/>
					</div>
					<div className="settings-security__ops">
						<Button
							variant="primary"
							loading={pending}
							disabled={!current || !next}
							onClick={() => void submit()}
						>
							修改密码
						</Button>
						<Button onClick={() => void logout()}>退出登录</Button>
					</div>
				</>
			)}
		</Section>
	);
}

const TRI_STATE = [	{ value: 0, label: '跟随默认' },
	{ value: 1, label: '仅此类' },
	{ value: 2, label: '排除' }
];

function CategoriesSection() {
	const categories = useCategories();
	const patch = usePatchCategory();
	const create = useCreateCategory();
	const rename = useRenameCategory();
	const del = useDeleteCategory();
	const reorder = useReorderCategories();

	const [newName, setNewName] = useState('');
	const [renaming, setRenaming] = useState<Category | null>(null);
	const [renameText, setRenameText] = useState('');
	const [deleting, setDeleting] = useState<Category | null>(null);

	const cats = categories.data ?? [];

	function toastError(e: unknown) {
		if (e instanceof ApiError && e.status === 409) pushToast('分类名已存在');
		else pushToast(e instanceof Error ? e.message : '操作失败');
	}

	async function submitCreate() {
		const name = newName.trim();
		if (!name) return;
		try {
			await create.mutateAsync(name);
			setNewName('');
			pushToast('已新建分类', 'info');
		} catch (e) {
			toastError(e);
		}
	}

	async function submitRename() {
		if (!renaming) return;
		const name = renameText.trim();
		if (!name) return;
		try {
			await rename.mutateAsync({ id: renaming.id, name });
			setRenaming(null);
			pushToast('已改名', 'info');
		} catch (e) {
			toastError(e);
		}
	}

	function move(index: number, delta: number) {
		const ids = cats.map((c) => c.id);
		const j = index + delta;
		if (j < 0 || j >= ids.length) return;
		[ids[index], ids[j]] = [ids[j], ids[index]];
		reorder.mutate(ids);
	}

	return (
		<Section title="分类管理" desc="分类用来给书库分组">
			{categories.isPending ? (
				<div className="settings-cats">
					{Array.from({ length: 2 }, (_, i) => (
						<Skeleton key={i} height={72} />
					))}
				</div>
			) : (
				<div className="settings-cats">
					{cats.map((c, i) => (
						<div key={c.id} className="settings-cat">
							<div className="settings-cat__head">
								<span className="settings-cat__name">
									{c.name}
									<Badge>{c.mangaCount}</Badge>
								</span>
								<span className="settings-cat__ops">
									<IconButton
										label="改名"
										size="sm"
										onClick={() => {
											setRenaming(c);
											setRenameText(c.name);
										}}
									>
										<Pencil size={15} />
									</IconButton>
									<IconButton
										label="上移"
										size="sm"
										disabled={i === 0 || reorder.isPending}
										onClick={() => move(i, -1)}
									>
										<ArrowUp size={15} />
									</IconButton>
									<IconButton
										label="下移"
										size="sm"
										disabled={i === cats.length - 1 || reorder.isPending}
										onClick={() => move(i, 1)}
									>
										<ArrowDown size={15} />
									</IconButton>
									{!c.isDefault && (
										<IconButton
											label="删除分类"
											size="sm"
											variant="danger"
											onClick={() => setDeleting(c)}
										>
											<Trash2 size={15} />
										</IconButton>
									)}
								</span>
							</div>
							<div className="settings-cat__fields">
								<div className="settings-cat__field">
									<span className="settings-cat__label">更新</span>
									<Select
										value={String(c.includeInUpdate)}
										onChange={(e) =>
											patch.mutate({ id: c.id, includeInUpdate: Number(e.currentTarget.value) })
										}
									>
										{TRI_STATE.map((o) => (
											<option key={o.value} value={o.value}>
												{o.label}
											</option>
										))}
									</Select>
									<span className="settings-cat__hint">全库更新时哪些分类参与</span>
								</div>
								<div className="settings-cat__field">
									<span className="settings-cat__label">下载</span>
									<Select
										value={String(c.includeInDownload)}
										onChange={(e) =>
											patch.mutate({ id: c.id, includeInDownload: Number(e.currentTarget.value) })
										}
									>
										{TRI_STATE.map((o) => (
											<option key={o.value} value={o.value}>
												{o.label}
											</option>
										))}
									</Select>
									<span className="settings-cat__hint">新章节自动下载范围</span>
								</div>
							</div>
						</div>
					))}
				</div>
			)}

			<div className="settings-cat__create">
				<Input
					placeholder="新分类名"
					value={newName}
					onChange={(e) => setNewName(e.currentTarget.value)}
					onKeyDown={(e) => {
						if (e.key === 'Enter') void submitCreate();
					}}
				/>
				<Button
					variant="primary"
					loading={create.isPending}
					disabled={!newName.trim()}
					onClick={() => void submitCreate()}
				>
					新建分类
				</Button>
			</div>

			<Modal
				open={renaming !== null}
				title="分类改名"
				onClose={() => setRenaming(null)}
				footer={
					<>
						<Button onClick={() => setRenaming(null)}>取消</Button>
						<Button
							variant="primary"
							loading={rename.isPending}
							disabled={!renameText.trim()}
							onClick={() => void submitRename()}
						>
							保存
						</Button>
					</>
				}
			>
				<Input
					className="settings-modal-input"
					placeholder="分类名"
					value={renameText}
					onChange={(e) => setRenameText(e.currentTarget.value)}
					onKeyDown={(e) => {
						if (e.key === 'Enter') void submitRename();
					}}
				/>
			</Modal>

			<Modal
				open={deleting !== null}
				title="删除分类"
				onClose={() => setDeleting(null)}
				footer={
					<>
						<Button onClick={() => setDeleting(null)}>取消</Button>
						<Button
							variant="danger"
							loading={del.isPending}
							onClick={() => {
								if (!deleting) return;
								del.mutate(deleting.id, {
									onSuccess: () => {
										setDeleting(null);
										pushToast('已删除', 'info');
									}
								});
							}}
						>
							删除
						</Button>
					</>
				}
			>
				<p className="settings-modal-text">
					删除分类「{deleting?.name}」？其中的漫画保留在书库。
				</p>
			</Modal>
		</Section>
	);
}

interface KvRow {
	key: string;
	/** JSON 文本，保存时再解析 */
	text: string;
	isNew?: boolean;
}

function parseVal(text: string): unknown {
	try {
		return JSON.parse(text);
	} catch {
		// 不是合法 JSON 时按字符串存
		return text;
	}
}

function ServerSettingsSection() {
	const query = useServerSettings();
	const save = useSaveServerSettings();
	const [rows, setRows] = useState<KvRow[] | null>(null);
	const [newKey, setNewKey] = useState('');
	const [newVal, setNewVal] = useState('');

	useEffect(() => {
		if (query.data && rows === null) {
			setRows(
				Object.entries(query.data).map(([key, v]) => ({ key, text: JSON.stringify(v) ?? '' }))
			);
		}
	}, [query.data, rows]);

	async function onSave() {
		if (!rows) return;
		const body: Record<string, unknown> = {};
		for (const r of rows) {
			const k = r.key.trim();
			if (k) body[k] = parseVal(r.text);
		}
		const saved = await save.mutateAsync(body);
		setRows(Object.entries(saved).map(([key, v]) => ({ key, text: JSON.stringify(v) ?? '' })));
		pushToast('服务器设置已保存', 'info');
	}

	function addRow() {
		const key = newKey.trim();
		if (!key) return;
		setRows((cur) => [...(cur ?? []), { key, text: newVal, isNew: true }]);
		setNewKey('');
		setNewVal('');
	}

	return (
		<Section title="服务器设置" desc="服务端暂不消费这些键，仅作通用存储">
			{rows === null ? (
				<Skeleton height={48} />
			) : (
				<>
					{rows.length === 0 ? (
						<p className="faint settings-kv__empty">暂无配置项</p>
					) : (
						<div className="settings-kv">
							{rows.map((row, i) => (
								<div key={i} className="settings-kv__row">
									<Input
										className={row.isNew ? '' : 'settings-kv__key'}
										placeholder="键"
										value={row.key}
										readOnly={!row.isNew}
										onChange={(e) =>
											setRows((cur) =>
												(cur ?? []).map((r, j) => (j === i ? { ...r, key: e.currentTarget.value } : r))
											)
										}
									/>
									<Input
										placeholder="值（JSON，字符串可裸写）"
										value={row.text}
										onChange={(e) =>
											setRows((cur) =>
												(cur ?? []).map((r, j) => (j === i ? { ...r, text: e.currentTarget.value } : r))
											)
										}
									/>
									<IconButton
										label="删除"
										variant="danger"
										onClick={() => setRows((cur) => (cur ?? []).filter((_, j) => j !== i))}
									>
										<Trash2 size={16} />
									</IconButton>
								</div>
							))}
						</div>
					)}
					<div className="settings-kv__add">
						<Input placeholder="键" value={newKey} onChange={(e) => setNewKey(e.currentTarget.value)} />
						<Input
							placeholder="值（JSON，字符串可裸写）"
							value={newVal}
							onChange={(e) => setNewVal(e.currentTarget.value)}
						/>
						<Button disabled={!newKey.trim()} onClick={addRow}>
							添加
						</Button>
					</div>
					<div className="settings-kv__save">
						<Button variant="primary" loading={save.isPending} onClick={() => void onSave()}>
							保存
						</Button>
					</div>
				</>
			)}
		</Section>
	);
}

const BACKUP_SECTIONS = [
	{ key: 'library', label: '书库' },
	{ key: 'categories', label: '分类' },
	{ key: 'progress', label: '阅读进度' },
	{ key: 'settings', label: '服务器设置' },
	{ key: 'sourceSettings', label: '源设置' }
] as const;

interface BackupPreview {
	doc: unknown;
	createdAt: number | null;
	counts: Record<string, number>;
}

function previewCounts(doc: Record<string, unknown>): Record<string, number> {
	const count = (key: string): number => {
		const v = doc[key];
		if (Array.isArray(v)) return v.length;
		if (v && typeof v === 'object') return Object.keys(v).length;
		return 0;
	};
	const sourceSettings = doc['sourceSettings'];
	const credentialCount =
		sourceSettings && typeof sourceSettings === 'object'
			? Object.values(sourceSettings).reduce(
					(n, v) => n + (v && typeof v === 'object' ? Object.keys(v).length : 0),
					0
				)
			: 0;
	return {
		library: count('library'),
		categories: count('categories'),
		progress: count('progress'),
		settings: count('settings'),
		sourceSettings: credentialCount
	};
}

function BackupSection() {
	const restore = useRestoreBackup();
	const fileRef = useRef<HTMLInputElement>(null);
	const [preview, setPreview] = useState<BackupPreview | null>(null);
	const [picked, setPicked] = useState<string[]>([]);
	const [result, setResult] = useState<BackupRestoreResult | null>(null);

	async function onFile(e: React.ChangeEvent<HTMLInputElement>) {
		const file = e.currentTarget.files?.[0];
		// 清空 value 让同一文件能再次触发 change
		e.currentTarget.value = '';
		if (!file) return;
		let doc: unknown;
		try {
			doc = JSON.parse(await file.text());
		} catch {
			pushToast('备份文件不是合法 JSON');
			return;
		}
		const version = (doc as { version?: unknown } | null)?.version;
		if (version !== 1) {
			pushToast(`备份版本不受支持：${typeof version === 'number' ? version : '缺失'}`);
			return;
		}
		const createdAt = (doc as { createdAt?: unknown }).createdAt;
		setPreview({
			doc,
			createdAt: typeof createdAt === 'number' ? createdAt : null,
			counts: previewCounts(doc as Record<string, unknown>)
		});
		setPicked(BACKUP_SECTIONS.map((s) => s.key));
		setResult(null);
	}

	function toggle(key: string, on: boolean) {
		setPicked((cur) => (on ? [...cur, key] : cur.filter((k) => k !== key)));
	}

	async function confirmRestore() {
		if (!preview || picked.length === 0) return;
		try {
			const res = await restore.mutateAsync({ doc: preview.doc, sections: picked });
			setResult(res);
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeModal() {
		setPreview(null);
		setResult(null);
	}

	return (
		<Section
			title="备份"
			desc="导出书库、分类、进度与设置用于迁移；备份文件含源设置（可能有源站账号密码），请妥善保管"
		>
			<div className="settings-backup__ops">
				<a className="btn settings-backup__download" href={backupUrl()} download>
					<Download size={15} />
					下载备份
				</a>
				<Button onClick={() => fileRef.current?.click()}>
					<Upload size={15} />
					上传备份恢复
				</Button>
				<input
					ref={fileRef}
					type="file"
					accept="application/json,.json"
					hidden
					onChange={(e) => void onFile(e)}
				/>
			</div>

			<Modal
				open={preview !== null}
				title="恢复备份"
				onClose={closeModal}
				footer={
					result ? (
						<Button variant="primary" onClick={closeModal}>
							完成
						</Button>
					) : (
						<>
							<Button onClick={closeModal}>取消</Button>
							<Button
								variant="danger"
								loading={restore.isPending}
								disabled={picked.length === 0}
								onClick={() => void confirmRestore()}
							>
								导入
							</Button>
						</>
					)
				}
			>
				{result ? (
					<div className="settings-backup__result">
						<p className="settings-modal-text">
							已恢复：书库 {result.restored.library} · 分类 {result.restored.categories} · 进度{' '}
							{result.restored.progress} · 设置 {result.restored.settings} · 源设置{' '}
							{result.restored.sourceSettings}
						</p>
						{result.warnings.length === 0 ? (
							<p className="faint">全部条目均已恢复，无跳过项</p>
						) : (
							<>
								<p className="settings-modal-text">以下条目被跳过：</p>
								<ul className="settings-backup__warnings">
									{result.warnings.map((w, i) => (
										<li key={i}>{w}</li>
									))}
								</ul>
							</>
						)}
					</div>
				) : (
					preview && (
						<>
							<p className="settings-modal-text">
								{preview.createdAt
									? `备份创建于 ${new Date(preview.createdAt * 1000).toLocaleString()}，选择要恢复的内容：`
									: '选择要恢复的内容：'}
							</p>
							<div className="settings-backup__sections">
								{BACKUP_SECTIONS.map((s) => (
									<label key={s.key} className="settings-backup__section">
										<input
											type="checkbox"
											checked={picked.includes(s.key)}
											onChange={(e) => toggle(s.key, e.currentTarget.checked)}
										/>
										{s.label}（{preview.counts[s.key] ?? 0}）
									</label>
								))}
							</div>
							<p className="settings-backup__warn">
								导入会按所选内容覆盖当前实例的对应状态，此操作不可撤销；备份可能包含源站账号密码，请确认文件来源可信。
							</p>
						</>
					)
				)}
			</Modal>
		</Section>
	);
}

function AboutSection() {
	return (
		<Section title="关于" desc="接口入口与项目地址">
			<div className="settings-about">
				<p>
					<strong>Sumidoku</strong> · 自建漫画服务器
				</p>
				<p className="muted">REST API：/api/v1 · OPDS：/opds/v1.2</p>
				<p className="faint">
					OPDS 客户端用 Basic 认证，密码与 web 界面相同（配置 SUMIDOKU_AUTH_PASSWORD 启用）
				</p>
				<a
					className="setting-link"
					href="https://github.com/Sumidoku/Sumidoku"
					target="_blank"
					rel="noreferrer"
				>
					GitHub
					<ExternalLink size={13} />
				</a>
			</div>
		</Section>
	);
}

export default function SettingsPage() {
	return (
		<div className="page container settings-page">
			<PageHeader title="设置" />
			<AppearanceSection />
			<SecuritySection />
			<CategoriesSection />
			<ServerSettingsSection />
			<BackupSection />
			<AboutSection />
		</div>
	);
}
