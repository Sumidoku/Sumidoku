import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import {
	ArrowDown,
	ArrowUp,
	Download,
	ExternalLink,
	FileUp,
	Pencil,
	RotateCw,
	Trash2,
	Upload
} from 'lucide-react';
import { ApiError, api, backupUrl, clearToken } from '../api/client';
import {
	useBackupFiles,
	useCategories,
	useCreateCategory,
	useDeleteCategory,
	useImportAidoku,
	useImportSuwayomi,
	usePatchCategory,
	useRenameCategory,
	useReorderCategories,
	useRestoreBackup,
	useRestoreNamedBackup,
	useSaveServerSettings,
	useServerSettings,
	useSources
} from '../api/hooks';
import type {
	AidokuImportResult,
	AidokuImportSkipped,
	BackupFileEntry,
	BackupRestoreResult,
	Category,
	SuwayomiImportResult,
	SuwayomiImportSkipped,
	SuwayomiSourceGroup
} from '../api/types';
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
	Switch,
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
	// 旧版设置页把「滚动」存成了 vertical，阅读器只认 continuous，读入时归一
	const [mode, setMode] = useState(() => {
		const m = stored(READER_MODE_KEY, 'paged');
		return m === 'vertical' ? 'continuous' : m;
	});
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
					<option value="continuous">滚动</option>
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

function IncognitoSection() {
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const enabled = settings.data?.['incognito.enabled'] === true;

	return (
		<Section
			title="隐身阅读"
			desc="开启后阅读进度照常保存，但不写入阅读历史（历史页与 OPDS 目录）；漫画详情页可对单部开启"
		>
			{settings.isPending ? (
				<Skeleton height={48} />
			) : (
				<label className="settings-backup__section">
					<Switch
						checked={enabled}
						disabled={save.isPending}
						onChange={(on) => save.mutate({ 'incognito.enabled': on })}
						label="全局隐身阅读"
					/>
					全局隐身阅读
				</label>
			)}
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

function LocalSourceSection() {
	const settings = useServerSettings();
	// localDir 是服务端注入的只读字段（config 分层值），不是 setting 表键
	const dir = settings.data?.['localDir'];

	return (
		<Section
			title="本地源"
			desc="直接阅读服务器本地目录里的漫画（浏览页的 Local 源）；目录只读展示，由配置文件 local_dir 或 SUMIDOKU_LOCAL_DIR 设置，改动重启后生效"
		>
			{settings.isPending ? (
				<Skeleton height={24} />
			) : (
				<div className="settings-row">
					<span className="settings-row__label">本地目录</span>
					<code>{typeof dir === 'string' ? dir : '未知'}</code>
				</div>
			)}
		</Section>
	);
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
				// localDir 是只读注入字段，不进键值编辑（见「本地源」区）
				Object.entries(query.data)
					.filter(([key]) => key !== 'localDir')
					.map(([key, v]) => ({ key, text: JSON.stringify(v) ?? '' }))
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
		<Section title="服务器设置" desc="通用键值存储；backup.* 键由「自动备份」区读写，请在那里修改">
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
										onChange={(e) => {
											// 更新函数延后执行时 currentTarget 已被置 null，必须先取值
											const v = e.currentTarget.value;
											setRows((cur) => (cur ?? []).map((r, j) => (j === i ? { ...r, key: v } : r)));
										}}
									/>
									<Input
										placeholder="值（JSON，字符串可裸写）"
										value={row.text}
										onChange={(e) => {
											const v = e.currentTarget.value;
											setRows((cur) => (cur ?? []).map((r, j) => (j === i ? { ...r, text: v } : r)));
										}}
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

function BackupResultView({ result }: { result: BackupRestoreResult }) {
	return (
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
	);
}

function aidokuSkippedLabel(s: AidokuImportSkipped): string {
	const what = s.section === 'library' ? '书库条目' : '阅读进度';
	const chapter = s.chapterKey ? ` / 章节 ${s.chapterKey}` : '';
	return `${what} ${s.sourceKey}/${s.mangaKey}${chapter}`;
}

function AidokuImportResultView({ result }: { result: AidokuImportResult }) {
	return (
		<div className="settings-backup__result">
			<p className="settings-modal-text">
				已导入：书库 {result.imported.library} · 分类 {result.imported.categories} · 进度{' '}
				{result.imported.progress}
			</p>
			{result.skipped.length === 0 ? (
				<p className="faint">全部条目均已导入，无跳过项</p>
			) : (
				<>
					<p className="settings-modal-text">以下条目被跳过（未安装对应源）：</p>
					<ul className="settings-backup__warnings">
						{result.skipped.map((s, i) => (
							<li key={i}>{aidokuSkippedLabel(s)}</li>
						))}
					</ul>
				</>
			)}
		</div>
	);
}

function suwayomiSkippedLabel(s: SuwayomiImportSkipped): string {
	const what = s.section === 'library' ? '书库条目' : '阅读进度';
	const name = s.mihonSourceName || `源 ${s.mihonSourceId}`;
	return `${what} ${name}/${s.mangaKey}`;
}

function suwayomiGroupName(g: SuwayomiSourceGroup): string {
	return g.mihonSourceName || `未知源 ${g.mihonSourceId}`;
}

/** unmatched 组的映射下拉：同名候选排前，其余已装源在后；留空即整组跳过 */
function SuwayomiMappingSelect({
	group,
	value,
	onChange
}: {
	group: SuwayomiSourceGroup;
	value: string;
	onChange: (key: string) => void;
}) {
	const sources = useSources();
	const candidateKeys = new Set(group.candidates.map((c) => c.key));
	const rest = (sources.data ?? []).filter((s) => !candidateKeys.has(s.key));
	return (
		<Select value={value} onChange={(e) => onChange(e.currentTarget.value)}>
			<option value="">跳过该源</option>
			{group.candidates.map((c) => (
				<option key={c.key} value={c.key}>
					{c.name}（{c.key}）
				</option>
			))}
			{rest.map((s) => (
				<option key={s.key} value={s.key}>
					{s.name}（{s.key}）
				</option>
			))}
		</Select>
	);
}

function SuwayomiImportResultView({ result }: { result: SuwayomiImportResult }) {
	return (
		<div className="settings-backup__result">
			<p className="settings-modal-text">
				已导入：书库 {result.imported.library} · 分类 {result.imported.categories} · 进度{' '}
				{result.imported.progress}
			</p>
			<p className="settings-modal-text">
				{result.sources
					.map((g) => {
						const label = suwayomiGroupName(g);
						if (g.match === 'unmatched') return `${label}（${g.mangaCount} 部，未映射已跳过）`;
						const how = g.match === 'auto' ? '自动匹配' : '手动映射';
						return `${label}（${g.mangaCount} 部，${how}到 ${g.aidokuSourceName ?? g.aidokuSourceKey}）`;
					})
					.join('；')}
			</p>
			{result.skipped.length > 0 && (
				<>
					<p className="settings-modal-text">以下条目被跳过：</p>
					<ul className="settings-backup__warnings">
						{result.skipped.map((s, i) => (
							<li key={i}>{suwayomiSkippedLabel(s)}</li>
						))}
					</ul>
				</>
			)}
		</div>
	);
}

function BackupSection() {
	const restore = useRestoreBackup();
	const fileRef = useRef<HTMLInputElement>(null);
	const [preview, setPreview] = useState<BackupPreview | null>(null);
	const [picked, setPicked] = useState<string[]>([]);
	const [result, setResult] = useState<BackupRestoreResult | null>(null);
	const importAidoku = useImportAidoku();
	const aibFileRef = useRef<HTMLInputElement>(null);
	const [aib, setAib] = useState<{ file: File; preview: AidokuImportResult } | null>(null);
	const [aibResult, setAibResult] = useState<AidokuImportResult | null>(null);
	const importSuwayomi = useImportSuwayomi();
	const swFileRef = useRef<HTMLInputElement>(null);
	const [sw, setSw] = useState<{ file: File; preview: SuwayomiImportResult } | null>(null);
	const [swMapping, setSwMapping] = useState<Record<string, string>>({});
	const [swResult, setSwResult] = useState<SuwayomiImportResult | null>(null);

	async function onSwFile(e: React.ChangeEvent<HTMLInputElement>) {
		const file = e.currentTarget.files?.[0];
		e.currentTarget.value = '';
		if (!file) return;
		try {
			const preview = await importSuwayomi.mutateAsync({ file, dryRun: true });
			setSw({ file, preview });
			setSwMapping({});
			setSwResult(null);
		} catch {
			// api() 已弹错误提示
		}
	}

	function setSwSource(mihonId: string, key: string) {
		setSwMapping((cur) => {
			const next = { ...cur };
			if (key) next[mihonId] = key;
			else delete next[mihonId];
			return next;
		});
	}

	async function confirmSwImport() {
		if (!sw) return;
		try {
			const res = await importSuwayomi.mutateAsync({ file: sw.file, dryRun: false, mapping: swMapping });
			setSwResult(res);
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeSwModal() {
		setSw(null);
		setSwMapping({});
		setSwResult(null);
	}

	async function onAibFile(e: React.ChangeEvent<HTMLInputElement>) {
		const file = e.currentTarget.files?.[0];
		e.currentTarget.value = '';
		if (!file) return;
		try {
			const dryPreview = await importAidoku.mutateAsync({ file, dryRun: true });
			setAib({ file, preview: dryPreview });
			setAibResult(null);
		} catch {
			// api() 已弹错误提示
		}
	}

	async function confirmAibImport() {
		if (!aib) return;
		try {
			const res = await importAidoku.mutateAsync({ file: aib.file, dryRun: false });
			setAibResult(res);
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeAibModal() {
		setAib(null);
		setAibResult(null);
	}

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
			desc="导出书库、分类、进度与设置用于迁移；备份文件含源设置（可能有源站账号密码），请妥善保管；也可导入 Aidoku App 的 .aib 或 Suwayomi/Mihon 的 .tachibk 备份（书库、分类、阅读进度，增量合并不覆盖现状）"
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
				<Button loading={importAidoku.isPending && !aib} onClick={() => aibFileRef.current?.click()}>
					<FileUp size={15} />
					从 Aidoku 导入
				</Button>
				<input
					ref={aibFileRef}
					type="file"
					accept=".aib"
					hidden
					onChange={(e) => void onAibFile(e)}
				/>
				<Button loading={importSuwayomi.isPending && !sw} onClick={() => swFileRef.current?.click()}>
					<FileUp size={15} />
					从 Suwayomi 导入
				</Button>
				<input
					ref={swFileRef}
					type="file"
					accept=".tachibk,.gz,.proto"
					hidden
					onChange={(e) => void onSwFile(e)}
				/>
			</div>

			<Modal
				open={sw !== null}
				title="从 Suwayomi 导入"
				onClose={closeSwModal}
				footer={
					swResult ? (
						<Button variant="primary" onClick={closeSwModal}>
							完成
						</Button>
					) : (
						<>
							<Button onClick={closeSwModal}>取消</Button>
							<Button variant="primary" loading={importSuwayomi.isPending} onClick={() => void confirmSwImport()}>
								确认导入
							</Button>
						</>
					)
				}
			>
				{swResult ? (
					<SuwayomiImportResultView result={swResult} />
				) : (
					sw && (
						<>
							<p className="settings-modal-text">
								将导入：书库 {sw.preview.imported.library} · 分类 {sw.preview.imported.categories} ·
								进度 {sw.preview.imported.progress}
							</p>
							<p className="settings-modal-text">备份里的源与已装源的对应关系：</p>
							<ul className="settings-backup__mapping">
								{sw.preview.sources.map((g) => (
									<li key={g.mihonSourceId} className="settings-backup__mapping-row">
										<span className="settings-backup__mapping-name">
											{suwayomiGroupName(g)}（{g.mangaCount} 部）
										</span>
										{g.match === 'unmatched' ? (
											<SuwayomiMappingSelect
												group={g}
												value={swMapping[g.mihonSourceId] ?? ''}
												onChange={(key) => setSwSource(g.mihonSourceId, key)}
											/>
										) : (
											<span className="faint">自动匹配：{g.aidokuSourceName ?? g.aidokuSourceKey}</span>
										)}
									</li>
								))}
							</ul>
							{sw.preview.skipped.length > 0 && (
								<p className="settings-modal-text">
									未选择映射的源将整组跳过（{sw.preview.skipped.length} 个条目）。
								</p>
							)}
							<p className="settings-backup__warn">
								导入是增量合并：已存在的书库条目与分类保持现状，阅读进度取较大值；此操作不可撤销。只有唯一同名的源才会自动匹配，其余需手动指定。
							</p>
						</>
					)
				)}
			</Modal>

			<Modal
				open={aib !== null}
				title="从 Aidoku 导入"
				onClose={closeAibModal}
				footer={
					aibResult ? (
						<Button variant="primary" onClick={closeAibModal}>
							完成
						</Button>
					) : (
						<>
							<Button onClick={closeAibModal}>取消</Button>
							<Button variant="primary" loading={importAidoku.isPending} onClick={() => void confirmAibImport()}>
								确认导入
							</Button>
						</>
					)
				}
			>
				{aibResult ? (
					<AidokuImportResultView result={aibResult} />
				) : (
					aib && (
						<>
							<p className="settings-modal-text">
								将导入：书库 {aib.preview.imported.library} · 分类 {aib.preview.imported.categories} ·
								进度 {aib.preview.imported.progress}
							</p>
							{aib.preview.skipped.length > 0 && (
								<>
									<p className="settings-modal-text">以下条目因未安装对应源将被跳过：</p>
									<ul className="settings-backup__warnings">
										{aib.preview.skipped.map((s, i) => (
											<li key={i}>{aidokuSkippedLabel(s)}</li>
										))}
									</ul>
								</>
							)}
							<p className="settings-backup__warn">
								导入是增量合并：已存在的书库条目与分类保持现状，阅读进度取较大值；此操作不可撤销。
							</p>
						</>
					)
				)}
			</Modal>

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
					<BackupResultView result={result} />
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

function formatSize(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function ScheduledBackupSection() {
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const backups = useBackupFiles();
	const restore = useRestoreNamedBackup();
	const [confirming, setConfirming] = useState<BackupFileEntry | null>(null);
	const [result, setResult] = useState<BackupRestoreResult | null>(null);
	const [intervalText, setIntervalText] = useState<string | null>(null);
	const [retentionText, setRetentionText] = useState<string | null>(null);

	useEffect(() => {
		if (!settings.data) return;
		if (intervalText === null)
			setIntervalText(String(Number(settings.data['backup.interval_hours'] ?? 24)));
		if (retentionText === null)
			setRetentionText(String(Number(settings.data['backup.retention_count'] ?? 10)));
	}, [settings.data, intervalText, retentionText]);

	const data = settings.data;
	if (!data || intervalText === null || retentionText === null) {
		return (
			<Section
				title="自动备份"
				desc="按周期把全量备份写入服务器目录；超出保留份数的最老备份自动清理，最新一份始终保留"
			>
				<Skeleton height={48} />
			</Section>
		);
	}

	const enabled = data['backup.enabled'] === true;
	const lastRun = data['backup.last_run'];
	const files = backups.data?.files ?? [];

	function commitNumber(
		key: 'backup.interval_hours' | 'backup.retention_count',
		text: string,
		fallback: number,
		min: number,
		reset: (v: string) => void
	) {
		const n = Number(text);
		if (!Number.isInteger(n) || n < min) {
			pushToast(`请输入不小于 ${min} 的整数`);
			reset(String(fallback));
			return;
		}
		if (n !== fallback) save.mutate({ [key]: n });
	}

	const intervalHours = Number(data['backup.interval_hours'] ?? 24);
	const retentionCount = Number(data['backup.retention_count'] ?? 10);

	async function confirmRestore() {
		if (!confirming) return;
		try {
			const res = await restore.mutateAsync(confirming.name);
			setResult(res);
			setConfirming(null);
		} catch {
			// api() 已弹错误提示
		}
	}

	function closeModal() {
		setConfirming(null);
		setResult(null);
	}

	return (
		<Section
			title="自动备份"
			desc="按周期把全量备份写入服务器目录；超出保留份数的最老备份自动清理，最新一份始终保留"
		>
			<label className="settings-backup__section">
				<input
					type="checkbox"
					checked={enabled}
					onChange={(e) => save.mutate({ 'backup.enabled': e.currentTarget.checked })}
				/>
				启用自动备份
			</label>
			<div className="settings-row">
				<span className="settings-row__label">备份间隔（小时）</span>
				<Input
					className="settings-backup__num"
					type="number"
					min={1}
					value={intervalText}
					onChange={(e) => setIntervalText(e.currentTarget.value)}
					onBlur={() =>
						commitNumber('backup.interval_hours', intervalText, intervalHours, 1, setIntervalText)
					}
					onKeyDown={(e) => {
						if (e.key === 'Enter')
							commitNumber('backup.interval_hours', intervalText, intervalHours, 1, setIntervalText);
					}}
				/>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">保留份数</span>
				<Input
					className="settings-backup__num"
					type="number"
					min={0}
					value={retentionText}
					onChange={(e) => setRetentionText(e.currentTarget.value)}
					onBlur={() =>
						commitNumber('backup.retention_count', retentionText, retentionCount, 0, setRetentionText)
					}
					onKeyDown={(e) => {
						if (e.key === 'Enter')
							commitNumber(
								'backup.retention_count',
								retentionText,
								retentionCount,
								0,
								setRetentionText
							);
					}}
				/>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">备份目录</span>
				<span className="settings-backup__dir">{backups.data?.dir ?? '…'}</span>
			</div>
			<p className="faint settings-backup__hint">
				目录可用 SUMIDOKU_BACKUP_DIR 或设置项 backup.dir 修改；上次备份：
				{typeof lastRun === 'number' ? new Date(lastRun * 1000).toLocaleString() : '从未'}
			</p>

			<div className="settings-backup__files-head">
				<span className="settings-row__label">已有备份</span>
				<IconButton label="刷新" size="sm" onClick={() => void backups.refetch()}>
					<RotateCw size={14} />
				</IconButton>
			</div>
			{files.length === 0 ? (
				<p className="faint">暂无备份</p>
			) : (
				<div className="settings-backup__files">
					{files.map((f) => (
						<div key={f.name} className="settings-backup__file">
							<div className="settings-backup__file-meta">
								<span>{new Date(f.createdAt * 1000).toLocaleString()}</span>
								<span className="faint">{formatSize(f.size)}</span>
							</div>
							<Button size="sm" onClick={() => setConfirming(f)}>
								恢复
							</Button>
						</div>
					))}
				</div>
			)}

			<Modal
				open={confirming !== null || result !== null}
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
							<Button variant="danger" loading={restore.isPending} onClick={() => void confirmRestore()}>
								恢复
							</Button>
						</>
					)
				}
			>
				{result ? (
					<BackupResultView result={result} />
				) : (
					confirming && (
						<>
							<p className="settings-modal-text">
								将从 {new Date(confirming.createdAt * 1000).toLocaleString()}{' '}
								创建的备份恢复，覆盖当前实例的书库、分类、进度与全部设置，此操作不可撤销。
							</p>
							<p className="settings-backup__warn">
								备份可能包含源站账号密码，请确认文件来源可信。
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
			<IncognitoSection />
			<SecuritySection />
			<CategoriesSection />
			<LocalSourceSection />
			<ServerSettingsSection />
			<BackupSection />
			<ScheduledBackupSection />
			<AboutSection />
		</div>
	);
}
