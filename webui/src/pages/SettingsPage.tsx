import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import {
	ArrowDown,
	ArrowUp,
	ChevronDown,
	ExternalLink,
	Pencil,
	Trash2
} from 'lucide-react';
import { ApiError } from '../api/client';
import {
	useCategories,
	useCreateCategory,
	useDeleteCategory,
	usePatchCategory,
	useRenameCategory,
	useReorderCategories,
	useSaveServerSettings,
	useSaveSourceSettings,
	useServerSettings,
	useSourceSettings,
	useSources
} from '../api/hooks';
import type { Category, Setting } from '../api/types';
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
	Spinner,
	pushToast
} from '../components';
import { SettingItem } from './settings/SettingItem';
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

const TRI_STATE = [
	{ value: 0, label: '跟随默认' },
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

/** 摊平 group/page 里的嵌套设置，按 key 找配置项用 */
function flattenSettings(list: Setting[]): Setting[] {
	return list.flatMap((s) => {
		const v = s.value;
		if (typeof v === 'object' && 'Group' in v) return flattenSettings(v.Group.items);
		if (typeof v === 'object' && 'Page' in v) return flattenSettings(v.Page.items);
		return [s];
	});
}

function SourceSettingsPanel({ sourceId }: { sourceId: number }) {
	const query = useSourceSettings(sourceId);
	const save = useSaveSourceSettings(sourceId);
	const [edits, setEdits] = useState<Record<string, unknown>>({});

	if (query.isPending) {
		return (
			<div className="settings-source__loading">
				<Spinner /> 加载中…
			</div>
		);
	}
	const data = query.data;
	if (!data) return <p className="faint settings-source__empty">设置加载失败</p>;
	if (!data.settings || data.settings.length === 0) {
		return <p className="faint settings-source__empty">该源没有可配置项</p>;
	}

	const hasEdits = Object.keys(edits).length > 0;

	async function onSave() {
		if (!hasEdits || !data?.settings) return;
		const keys = Object.keys(edits);
		await save.mutateAsync(edits);
		setEdits({});
		pushToast('已保存', 'info');
		const touched = flattenSettings(data.settings).filter((s) => keys.includes(s.key));
		if (touched.some((s) => s.refreshes && s.refreshes.length > 0)) {
			pushToast('该源会在后台刷新设置', 'info');
		}
	}

	return (
		<div className="settings-source__body">
			{data.settings.map((setting) => (
				<SettingItem
					key={setting.key}
					setting={setting}
					values={data.values}
					edits={edits}
					onEdit={(k, v) => setEdits((cur) => ({ ...cur, [k]: v }))}
				/>
			))}
			<div className="settings-source__save">
				<Button
					variant="primary"
					size="sm"
					loading={save.isPending}
					disabled={!hasEdits}
					onClick={() => void onSave()}
				>
					保存
				</Button>
			</div>
		</div>
	);
}

function SourcesSection() {
	const sources = useSources();
	const [openId, setOpenId] = useState<number | null>(null);
	const list = sources.data ?? [];

	return (
		<Section title="源设置" desc="配置项由各源包提供">
			{sources.isPending ? (
				<Skeleton height={48} />
			) : list.length === 0 ? (
				<p className="faint settings-source__empty">还没有安装任何源</p>
			) : (
				<div className="settings-sources">
					{list.map((s) => {
						const open = openId === s.id;
						return (
							<div key={s.id} className="settings-source">
								<button
									type="button"
									className="settings-source__head"
									onClick={() => setOpenId(open ? null : s.id)}
								>
									<span className="settings-source__name">
										{s.name}
										<span className="settings-source__version">v{s.version}</span>
									</span>
									<ChevronDown
										size={16}
										className={
											open ? 'settings-source__chev settings-source__chev--open' : 'settings-source__chev'
										}
									/>
								</button>
								{open && <SourceSettingsPanel sourceId={s.id} />}
							</div>
						);
					})}
				</div>
			)}
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

function AboutSection() {
	return (
		<Section title="关于" desc="接口入口与项目地址">
			<div className="settings-about">
				<p>
					<strong>Sumidoku</strong> · 自建漫画服务器
				</p>
				<p className="muted">REST API：/api/v1 · OPDS：/opds/v1.2</p>
				<p className="faint">
					OPDS 客户端用 Basic 认证，需在服务端配置 SUMIDOKU_BASIC_PASSWORD
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
			<CategoriesSection />
			<SourcesSection />
			<ServerSettingsSection />
			<AboutSection />
		</div>
	);
}
