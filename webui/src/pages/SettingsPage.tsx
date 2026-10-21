import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
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
	qk,
	queryClient,
	useBackupFiles,
	useCategories,
	useCreateCategory,
	useCreateFilterGroup,
	useDeleteCategory,
	useDeleteFilterGroup,
	useFilterGroups,
	useImportAidoku,
	useImportSuwayomi,
	useIsAdmin,
	usePatchCategory,
	useRenameCategory,
	useReorderCategories,
	useReorderFilterGroups,
	useRestoreBackup,
	useRestoreNamedBackup,
	useSaveServerSettings,
	useServerSettings,
	useSources,
	useUpdateFilterGroup,
	useWhoami
} from '../api/hooks';
import type {
	AidokuImportResult,
	AidokuImportSkipped,
	AuthStatus,
	BackupFileEntry,
	BackupRestoreResult,
	Category,
	FilterGroup,
	FilterGroupRule,
	FilterGroupRuleType,
	SuwayomiImportResult,
	SuwayomiImportSkipped,
	SuwayomiSourceGroup,
	TriStateSelection
} from '../api/types';
import { MANGA_CONTENT_RATING_LEVELS } from '../lib/source';
import { useTheme } from '../lib/theme';
import type { Theme } from '../lib/theme';
import { setLanguage, translateApiError } from '../i18n';
import type { Language } from '../i18n';
import { compareVersions } from '../lib/version';
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
	TriStateDropdown,
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
	const { t, i18n } = useTranslation();
	const [theme, setTheme] = useTheme();
	// The old settings page stored "scroll" as vertical, while the reader only knows continuous; normalize on read
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
			// Private mode blocks the write but the setting still switches, just without persisting
		}
	}

	return (
		<Section title={t('settings.appearanceTitle')} desc={t('settings.appearanceDesc')}>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.language')}</span>
				<Select
					value={i18n.language}
					onChange={(e) => setLanguage(e.currentTarget.value as Language)}
				>
					<option value="zh-CN">中文（简体）</option>
					<option value="en">English</option>
				</Select>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.theme')}</span>
				<SegmentedControl<Theme>
					options={[
						{ value: 'system', label: t('settings.themeSystem') },
						{ value: 'dark', label: t('settings.themeDark') },
						{ value: 'light', label: t('settings.themeLight') }
					]}
					value={theme}
					onChange={setTheme}
				/>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.readerMode')}</span>
				<Select
					value={mode}
					onChange={(e) => persist(READER_MODE_KEY, e.currentTarget.value, setMode)}
				>
					<option value="paged">{t('settings.modePaged')}</option>
					<option value="continuous">{t('settings.modeContinuous')}</option>
					<option value="webtoon">{t('settings.modeWebtoon')}</option>
				</Select>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.readerDirection')}</span>
				<Select value={dir} onChange={(e) => persist(READER_DIR_KEY, e.currentTarget.value, setDir)}>
					<option value="ltr">{t('settings.dirLtr')}</option>
					<option value="rtl">{t('settings.dirRtl')}</option>
				</Select>
			</div>
		</Section>
	);
}

function IncognitoSection() {
	const { t } = useTranslation();
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const enabled = settings.data?.['incognito.enabled'] === true;

	return (
		<Section
			title={t('settings.incognitoTitle')}
			desc={t('settings.incognitoDesc')}
		>
			{settings.isPending ? (
				<Skeleton height={48} />
			) : (
				<label className="settings-backup__section">
					<Switch
						checked={enabled}
						disabled={save.isPending}
						onChange={(on) => save.mutate({ 'incognito.enabled': on })}
						label={t('settings.incognitoGlobal')}
					/>
					{t('settings.incognitoGlobal')}
				</label>
			)}
		</Section>
	);
}

function SecuritySection() {
	const { t } = useTranslation();
	const navigate = useNavigate();
	const whoami = useWhoami();
	const [current, setCurrent] = useState('');
	const [next, setNext] = useState('');
	const [pending, setPending] = useState(false);

	async function submit() {
		if (!current || !next || pending) return;
		setPending(true);
		try {
			// Changing the password revokes all session tokens, so success forces a re-login
			await api('/auth/password', {
				method: 'PUT',
				body: { currentPassword: current, newPassword: next },
				silent: true
			});
			pushToast(t('settings.passwordChanged'), 'info');
			clearToken();
			navigate('/login');
		} catch (e) {
			pushToast(e instanceof ApiError && e.status === 401 ? t('settings.wrongPassword') : t('settings.changeFailed'));
		} finally {
			setPending(false);
		}
	}

	async function logout() {
		await api('/auth/logout', { method: 'POST', silent: true }).catch(() => {});
		clearToken();
		queryClient.setQueryData(qk.whoami, (cur: AuthStatus | undefined) =>
			cur ? { ...cur, user: null } : cur
		);
		navigate('/login');
	}

	const enabled = whoami.data === undefined ? null : whoami.data.enabled;
	const user = whoami.data?.user ?? null;

	return (
		<Section title={t('settings.securityTitle')} desc={t('settings.securityDesc')}>
			{enabled === null ? (
				<Skeleton height={48} />
			) : !enabled ? (
				<p className="faint settings-security__off">
					{t('settings.authDisabled')}
				</p>
			) : (
				<>
					{user && (
						<div className="settings-row">
							<span className="settings-row__label">{t('settings.currentAccount')}</span>
							<span className="settings-security__account">
								{user.username}
								<Badge>{user.role === 'admin' ? t('settings.roleAdmin') : t('settings.roleReader')}</Badge>
							</span>
						</div>
					)}
					<div className="settings-row">
						<span className="settings-row__label">{t('settings.currentPassword')}</span>
						<Input
							type="password"
							value={current}
							onChange={(e) => setCurrent(e.currentTarget.value)}
						/>
					</div>
					<div className="settings-row">
						<span className="settings-row__label">{t('settings.newPassword')}</span>
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
							{t('settings.changePassword')}
						</Button>
						<Button onClick={() => void logout()}>{t('settings.logout')}</Button>
					</div>
				</>
			)}
		</Section>
	);
}

const TRI_STATE = [	{ value: 0, labelKey: 'settings.triDefault' },
	{ value: 1, labelKey: 'settings.triOnly' },
	{ value: 2, labelKey: 'settings.triExclude' }
];

function CategoriesSection() {
	const { t } = useTranslation();
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
		if (e instanceof ApiError && e.status === 409) pushToast(t('settings.categoryExists'));
		else if (e instanceof ApiError) pushToast(translateApiError(e.code, e.message));
		else pushToast(e instanceof Error ? e.message : t('settings.operationFailed'));
	}

	async function submitCreate() {
		const name = newName.trim();
		if (!name) return;
		try {
			await create.mutateAsync(name);
			setNewName('');
			pushToast(t('settings.categoryCreated'), 'info');
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
			pushToast(t('settings.renamed'), 'info');
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
		<Section title={t('settings.categoriesTitle')} desc={t('settings.categoriesDesc')}>
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
										label={t('settings.rename')}
										size="sm"
										onClick={() => {
											setRenaming(c);
											setRenameText(c.name);
										}}
									>
										<Pencil size={15} />
									</IconButton>
									<IconButton
										label={t('settings.moveUp')}
										size="sm"
										disabled={i === 0 || reorder.isPending}
										onClick={() => move(i, -1)}
									>
										<ArrowUp size={15} />
									</IconButton>
									<IconButton
										label={t('settings.moveDown')}
										size="sm"
										disabled={i === cats.length - 1 || reorder.isPending}
										onClick={() => move(i, 1)}
									>
										<ArrowDown size={15} />
									</IconButton>
									{!c.isDefault && (
										<IconButton
											label={t('settings.deleteCategory')}
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
									<span className="settings-cat__label">{t('settings.update')}</span>
									<Select
										value={String(c.includeInUpdate)}
										onChange={(e) =>
											patch.mutate({ id: c.id, includeInUpdate: Number(e.currentTarget.value) })
										}
									>
										{TRI_STATE.map((o) => (
											<option key={o.value} value={o.value}>
												{t(o.labelKey)}
											</option>
										))}
									</Select>
									<span className="settings-cat__hint">{t('settings.updateHint')}</span>
								</div>
								<div className="settings-cat__field">
									<span className="settings-cat__label">{t('settings.download')}</span>
									<Select
										value={String(c.includeInDownload)}
										onChange={(e) =>
											patch.mutate({ id: c.id, includeInDownload: Number(e.currentTarget.value) })
										}
									>
										{TRI_STATE.map((o) => (
											<option key={o.value} value={o.value}>
												{t(o.labelKey)}
											</option>
										))}
									</Select>
									<span className="settings-cat__hint">{t('settings.downloadHint')}</span>
								</div>
							</div>
						</div>
					))}
				</div>
			)}

			<div className="settings-cat__create">
				<Input
					placeholder={t('settings.newCategoryPlaceholder')}
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
					{t('settings.createCategory')}
				</Button>
			</div>

			<Modal
				open={renaming !== null}
				title={t('settings.renameCategoryTitle')}
				onClose={() => setRenaming(null)}
				footer={
					<>
						<Button onClick={() => setRenaming(null)}>{t('common.cancel')}</Button>
						<Button
							variant="primary"
							loading={rename.isPending}
							disabled={!renameText.trim()}
							onClick={() => void submitRename()}
						>
							{t('common.save')}
						</Button>
					</>
				}
			>
				<Input
					className="settings-modal-input"
					placeholder={t('settings.categoryNamePlaceholder')}
					value={renameText}
					onChange={(e) => setRenameText(e.currentTarget.value)}
					onKeyDown={(e) => {
						if (e.key === 'Enter') void submitRename();
					}}
				/>
			</Modal>

			<Modal
				open={deleting !== null}
				title={t('settings.deleteCategory')}
				onClose={() => setDeleting(null)}
				footer={
					<>
						<Button onClick={() => setDeleting(null)}>{t('common.cancel')}</Button>
						<Button
							variant="danger"
							loading={del.isPending}
							onClick={() => {
								if (!deleting) return;
								del.mutate(deleting.id, {
									onSuccess: () => {
										setDeleting(null);
										pushToast(t('settings.deleted'), 'info');
									}
								});
							}}
						>
							{t('common.delete')}
						</Button>
					</>
				}
			>
				<p className="settings-modal-text">
					{t('settings.deleteCategoryConfirm', { name: deleting?.name })}
				</p>
			</Modal>
		</Section>
	);
}

function LibraryDisplaySection() {
	const { t } = useTranslation();
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const categories = useCategories();

	const defaultCategory = String(settings.data?.['library.default_category'] ?? 'all');
	const showUncategorized = settings.data?.['library.show_uncategorized'] !== false;

	return (
		<Section title={t('settings.libraryTitle')} desc={t('settings.libraryDesc')}>
			{settings.isPending ? (
				<Skeleton height={72} />
			) : (
				<>
					<div className="settings-row">
						<span className="settings-row__label">{t('settings.defaultCategory')}</span>
						<Select
							value={defaultCategory}
							disabled={save.isPending}
							onChange={(e) => save.mutate({ 'library.default_category': e.currentTarget.value })}
						>
							<option value="all">{t('settings.defaultCategoryAll')}</option>
							<option value="uncategorized">{t('settings.defaultCategoryUncategorized')}</option>
							{(categories.data ?? []).map((c) => (
								<option key={c.id} value={String(c.id)}>
									{c.name}
								</option>
							))}
						</Select>
					</div>
					<p className="settings-cat__hint">{t('settings.defaultCategoryHint')}</p>
					<label className="settings-backup__section">
						<Switch
							checked={showUncategorized}
							disabled={save.isPending}
							onChange={(on) => save.mutate({ 'library.show_uncategorized': on })}
							label={t('settings.showUncategorized')}
						/>
						{t('settings.showUncategorized')}
					</label>
				</>
			)}
		</Section>
	);
}

const LIBRARY_UPDATE_INTERVALS = [
	{ hours: 0, labelKey: 'settings.libraryUpdateNever' },
	{ hours: 12, labelKey: 'settings.libraryUpdate12Hours' },
	{ hours: 24, labelKey: 'settings.libraryUpdateDaily' },
	{ hours: 48, labelKey: 'settings.libraryUpdate2Days' },
	{ hours: 168, labelKey: 'settings.libraryUpdateWeekly' }
];
const LIBRARY_SKIP_RULES = [
	{ key: 'hasUnread', labelKey: 'settings.librarySkipHasUnread' },
	{ key: 'completed', labelKey: 'settings.librarySkipCompleted' },
	{ key: 'notStarted', labelKey: 'settings.librarySkipNotStarted' }
];

function LibraryUpdateSection() {
	const { t } = useTranslation();
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const intervalHours = Number(settings.data?.['library.update_interval_hours'] ?? 0);
	const storedSkip = settings.data?.['library.skip_titles'];
	const skipTitles = Array.isArray(storedSkip)
		? storedSkip.filter((value): value is string => typeof value === 'string')
		: [];
	const refreshMetadata = settings.data?.['library.refresh_metadata'] === true;

	function saveSkip(rule: string, on: boolean) {
		const changed = on ? [...skipTitles, rule] : skipTitles.filter((value) => value !== rule);
		const ordered = LIBRARY_SKIP_RULES.map(({ key }) => key).filter((key) => changed.includes(key));
		save.mutate({ 'library.skip_titles': ordered });
	}

	return (
		<Section title={t('settings.libraryUpdateTitle')} desc={t('settings.libraryUpdateDesc')}>
			{settings.isPending ? (
				<Skeleton height={180} />
			) : (
				<>
					<div className="settings-row">
						<span className="settings-row__label">{t('settings.libraryUpdateInterval')}</span>
						<Select
							value={String(intervalHours)}
							disabled={save.isPending}
							onChange={(e) => {
								const value = e.currentTarget.value;
								save.mutate({ 'library.update_interval_hours': Number(value) });
							}}
						>
							{LIBRARY_UPDATE_INTERVALS.map(({ hours, labelKey }) => (
								<option key={hours} value={hours}>
									{t(labelKey)}
								</option>
							))}
							{!LIBRARY_UPDATE_INTERVALS.some(({ hours }) => hours === intervalHours) && (
								<option value={intervalHours}>
									{t('settings.libraryUpdateCustom', { hours: intervalHours })}
								</option>
							)}
						</Select>
					</div>
					{LIBRARY_SKIP_RULES.map(({ key, labelKey }) => (
						<label key={key} className="settings-backup__section">
							<Switch
								checked={skipTitles.includes(key)}
								disabled={save.isPending}
								onChange={(on) => saveSkip(key, on)}
								label={t(labelKey)}
							/>
							{t(labelKey)}
						</label>
					))}
					<label className="settings-backup__section">
						<Switch
							checked={refreshMetadata}
							disabled={save.isPending}
							onChange={(on) => save.mutate({ 'library.refresh_metadata': on })}
							label={t('settings.libraryRefreshMetadata')}
						/>
						{t('settings.libraryRefreshMetadata')}
					</label>
				</>
			)}
		</Section>
	);
}

type FlagRuleType = 'downloaded' | 'has_unread' | 'started' | 'completed';
const FLAG_RULE_TYPES: { type: FlagRuleType; labelKey: string }[] = [
	{ type: 'downloaded', labelKey: 'settings.ruleDownloaded' },
	{ type: 'has_unread', labelKey: 'settings.ruleHasUnread' },
	{ type: 'started', labelKey: 'settings.ruleStarted' },
	{ type: 'completed', labelKey: 'settings.ruleCompleted' }
];

/** Editor state of one group: per-flag tri-states plus the three valued dimensions' tri-state selections */
interface GroupDraft {
	name: string;
	flags: Record<FlagRuleType, 0 | 1 | -1>;
	sources: TriStateSelection;
	ratings: TriStateSelection;
	categories: TriStateSelection;
}

const EMPTY_DRAFT: GroupDraft = {
	name: '',
	flags: { downloaded: 0, has_unread: 0, started: 0, completed: 0 },
	sources: {},
	ratings: {},
	categories: {}
};

/** Same tri-state cycle as the library page's chips: unselected → include → exclude → unselected */
function nextGroupTri(cur: 1 | -1 | 0): 1 | -1 | 0 {
	return cur === 0 ? 1 : cur === 1 ? -1 : 0;
}

function draftOf(group: FilterGroup): GroupDraft {
	const draft: GroupDraft = { ...EMPTY_DRAFT, name: group.name, flags: { ...EMPTY_DRAFT.flags } };
	for (const rule of group.rules) {
		const state = rule.exclude ? -1 : 1;
		if (rule.value === null) draft.flags[rule.type as FlagRuleType] = state;
		else if (rule.type === 'source') draft.sources[String(rule.value)] = state;
		else if (rule.type === 'content_rating') draft.ratings[String(rule.value)] = state;
		else if (rule.type === 'category') draft.categories[String(rule.value)] = state;
	}
	return draft;
}

function rulesOf(draft: GroupDraft): FilterGroupRule[] {
	const rules: FilterGroupRule[] = [];
	for (const { type } of FLAG_RULE_TYPES) {
		const state = draft.flags[type];
		if (state !== 0) rules.push({ type, value: null, exclude: state === -1 });
	}
	const push = (type: FilterGroupRuleType, sel: TriStateSelection) => {
		for (const [key, state] of Object.entries(sel)) {
			rules.push({ type, value: Number(key), exclude: state === -1 });
		}
	};
	push('source', draft.sources);
	push('content_rating', draft.ratings);
	push('category', draft.categories);
	return rules;
}

function FilterGroupsSection() {
	const { t } = useTranslation();
	const groups = useFilterGroups();
	const categories = useCategories();
	const sourcesQuery = useSources();
	const create = useCreateFilterGroup();
	const update = useUpdateFilterGroup();
	const del = useDeleteFilterGroup();
	const reorder = useReorderFilterGroups();

	const [editing, setEditing] = useState<{ id: number | null; draft: GroupDraft } | null>(null);
	const [deleting, setDeleting] = useState<FilterGroup | null>(null);

	const list = groups.data ?? [];

	function toastError(e: unknown) {
		if (e instanceof ApiError && e.status === 409) pushToast(t('settings.groupExists'));
		else if (e instanceof ApiError) pushToast(translateApiError(e.code, e.message));
		else pushToast(e instanceof Error ? e.message : t('settings.operationFailed'));
	}

	async function submitEditor() {
		if (!editing) return;
		const name = editing.draft.name.trim();
		if (!name) return;
		try {
			if (editing.id === null) {
				await create.mutateAsync({ name, rules: rulesOf(editing.draft) });
				pushToast(t('settings.groupCreated'), 'info');
			} else {
				await update.mutateAsync({ id: editing.id, name, rules: rulesOf(editing.draft) });
				pushToast(t('settings.groupSaved'), 'info');
			}
			setEditing(null);
		} catch (e) {
			toastError(e);
		}
	}

	function move(index: number, delta: number) {
		const ids = list.map((g) => g.id);
		const j = index + delta;
		if (j < 0 || j >= ids.length) return;
		[ids[index], ids[j]] = [ids[j], ids[index]];
		reorder.mutate(ids);
	}

	function cycleTri(dim: 'sources' | 'ratings' | 'categories', key: string) {
		setEditing((cur) => {
			if (!cur) return cur;
			const sel = { ...cur.draft[dim] };
			const next = nextGroupTri(sel[key] ?? 0);
			if (next === 0) delete sel[key];
			else sel[key] = next;
			return { ...cur, draft: { ...cur.draft, [dim]: sel } };
		});
	}

	const saving = create.isPending || update.isPending;

	return (
		<Section title={t('settings.filterGroupsTitle')} desc={t('settings.filterGroupsDesc')}>
			{groups.isPending ? (
				<div className="settings-cats">
					{Array.from({ length: 2 }, (_, i) => (
						<Skeleton key={i} height={56} />
					))}
				</div>
			) : (
				<div className="settings-cats">
					{list.map((g, i) => (
						<div key={g.id} className="settings-cat">
							<div className="settings-cat__head">
								<span className="settings-cat__name">
									{g.name}
									<Badge>{g.rules.length}</Badge>
								</span>
								<span className="settings-cat__ops">
									<IconButton
										label={t('settings.editGroup')}
										size="sm"
										onClick={() => setEditing({ id: g.id, draft: draftOf(g) })}
									>
										<Pencil size={15} />
									</IconButton>
									<IconButton
										label={t('settings.moveUp')}
										size="sm"
										disabled={i === 0 || reorder.isPending}
										onClick={() => move(i, -1)}
									>
										<ArrowUp size={15} />
									</IconButton>
									<IconButton
										label={t('settings.moveDown')}
										size="sm"
										disabled={i === list.length - 1 || reorder.isPending}
										onClick={() => move(i, 1)}
									>
										<ArrowDown size={15} />
									</IconButton>
									<IconButton
										label={t('settings.deleteGroup')}
										size="sm"
										variant="danger"
										onClick={() => setDeleting(g)}
									>
										<Trash2 size={15} />
									</IconButton>
								</span>
							</div>
						</div>
					))}
				</div>
			)}

			<div className="settings-cat__create">
				<Button variant="primary" onClick={() => setEditing({ id: null, draft: EMPTY_DRAFT })}>
					{t('settings.createGroup')}
				</Button>
			</div>

			<Modal
				open={editing !== null}
				title={editing?.id === null ? t('settings.createGroupTitle') : t('settings.editGroupTitle')}
				onClose={() => setEditing(null)}
				panelClassName="modal__panel--wide"
				footer={
					<>
						<Button onClick={() => setEditing(null)}>{t('common.cancel')}</Button>
						<Button
							variant="primary"
							loading={saving}
							disabled={!editing?.draft.name.trim()}
							onClick={() => void submitEditor()}
						>
							{t('common.save')}
						</Button>
					</>
				}
			>
				{editing && (
					<div className="settings-group-editor">
						<Input
							className="settings-modal-input"
							placeholder={t('settings.groupNamePlaceholder')}
							value={editing.draft.name}
							onChange={(e) => setEditing({ ...editing, draft: { ...editing.draft, name: e.currentTarget.value } })}
						/>
						{FLAG_RULE_TYPES.map(({ type, labelKey }) => (
							<div key={type} className="settings-cat__field">
								<span className="settings-cat__label">{t(labelKey)}</span>
								<SegmentedControl
									options={[
										{ value: 0, label: t('settings.ruleNone') },
										{ value: 1, label: t('settings.ruleInclude') },
										{ value: -1, label: t('settings.ruleExclude') }
									]}
									value={editing.draft.flags[type]}
									onChange={(v) =>
										setEditing({
											...editing,
											draft: { ...editing.draft, flags: { ...editing.draft.flags, [type]: v } }
										})
									}
								/>
							</div>
						))}
						<div className="settings-cat__field">
							<span className="settings-cat__label">{t('settings.ruleSources')}</span>
							<TriStateDropdown
								label={t('settings.ruleSources')}
								options={(sourcesQuery.data ?? []).map((s) => ({ key: String(s.id), label: s.name }))}
								states={editing.draft.sources}
								onCycle={(k) => cycleTri('sources', k)}
								onReset={() => setEditing({ ...editing, draft: { ...editing.draft, sources: {} } })}
							/>
						</div>
						<div className="settings-cat__field">
							<span className="settings-cat__label">{t('settings.ruleRatings')}</span>
							<TriStateDropdown
								label={t('settings.ruleRatings')}
								options={MANGA_CONTENT_RATING_LEVELS.map((l) => ({ key: String(l.value), label: l.label }))}
								states={editing.draft.ratings}
								onCycle={(k) => cycleTri('ratings', k)}
								onReset={() => setEditing({ ...editing, draft: { ...editing.draft, ratings: {} } })}
							/>
						</div>
						<div className="settings-cat__field">
							<span className="settings-cat__label">{t('settings.ruleCategories')}</span>
							<TriStateDropdown
								label={t('settings.ruleCategories')}
								options={(categories.data ?? []).map((c) => ({ key: String(c.id), label: c.name }))}
								states={editing.draft.categories}
								onCycle={(k) => cycleTri('categories', k)}
								onReset={() => setEditing({ ...editing, draft: { ...editing.draft, categories: {} } })}
							/>
						</div>
					</div>
				)}
			</Modal>

			<Modal
				open={deleting !== null}
				title={t('settings.deleteGroup')}
				onClose={() => setDeleting(null)}
				footer={
					<>
						<Button onClick={() => setDeleting(null)}>{t('common.cancel')}</Button>
						<Button
							variant="danger"
							loading={del.isPending}
							onClick={() => {
								if (!deleting) return;
								del.mutate(deleting.id, {
									onSuccess: () => {
										setDeleting(null);
										pushToast(t('settings.deleted'), 'info');
									}
								});
							}}
						>
							{t('common.delete')}
						</Button>
					</>
				}
			>
				<p className="settings-modal-text">
					{t('settings.deleteGroupConfirm', { name: deleting?.name })}
				</p>
			</Modal>
		</Section>
	);
}

interface KvRow {
	key: string;
	/** JSON text, parsed at save time */
	text: string;
	isNew?: boolean;
}

function parseVal(text: string): unknown {
	try {
		return JSON.parse(text);
	} catch {
		// Store as a string when it is not valid JSON
		return text;
	}
}

function LocalSourceSection() {
	const { t } = useTranslation();
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const [dirs, setDirs] = useState<string[] | null>(null);
	const [oneshots, setOneshots] = useState<string | null>(null);

	useEffect(() => {
		if (!settings.data || dirs !== null) return;
		const injected = settings.data['localDirs'];
		setDirs(Array.isArray(injected) ? injected.map(String) : []);
		const name = settings.data['localOneshotsDir'];
		setOneshots(typeof name === 'string' ? name : '');
	}, [settings.data, dirs]);

	if (dirs === null || oneshots === null) {
		return (
			<Section
				title={t('settings.localSourceTitle')}
				desc={t('settings.localSourceDesc')}
			>
				<Skeleton height={48} />
			</Section>
		);
	}

	// localDirs/localOneshotsDir are the server's read-only injections of the effective values; edits are
	// saved as the local.dirs / local.oneshots_dir overrides, and an empty value clears it (the config
	// layer takes over again)
	const effective = Array.isArray(settings.data?.['localDirs'])
		? (settings.data?.['localDirs'] as unknown[]).map(String)
		: [];
	const injectedOneshots = settings.data?.['localOneshotsDir'];
	const effectiveOneshots = typeof injectedOneshots === 'string' ? injectedOneshots : '';
	const cleaned = dirs.map((d) => d.trim()).filter(Boolean);
	const oneshotsName = oneshots.trim();
	const dirty = cleaned.join('\n') !== effective.join('\n') || oneshotsName !== effectiveOneshots;

	async function onSave() {
		try {
			const saved = await save.mutateAsync({
				'local.dirs': cleaned,
				'local.oneshots_dir': oneshotsName,
			});
			const injected = saved['localDirs'];
			setDirs(Array.isArray(injected) ? injected.map(String) : []);
			const name = saved['localOneshotsDir'];
			setOneshots(typeof name === 'string' ? name : '');
			pushToast(t('settings.localSourceSaved'), 'info');
		} catch (e) {
			if (e instanceof ApiError) pushToast(translateApiError(e.code, e.message));
		}
	}

	return (
		<Section
			title={t('settings.localSourceTitle')}
			desc={t('settings.localSourceDesc')}
		>
			{dirs.length > 0 && (
				<div className="settings-kv">
					{dirs.map((dir, i) => (
						<div key={i} className="settings-kv__row">
							<Input
								placeholder={t('settings.localDirPlaceholder')}
								value={dir}
								onChange={(e) => {
									// currentTarget is already null by the time the deferred updater runs, so take the value first
									const v = e.currentTarget.value;
									setDirs((cur) => (cur ?? []).map((d, j) => (j === i ? v : d)));
								}}
							/>
							<IconButton
								label={t('common.delete')}
								variant="danger"
								onClick={() => setDirs((cur) => (cur ?? []).filter((_, j) => j !== i))}
							>
								<Trash2 size={16} />
							</IconButton>
						</div>
					))}
				</div>
			)}
			<div className="settings-kv__add">
				<Button onClick={() => setDirs((cur) => [...(cur ?? []), ''])}>
					{t('settings.addLocalDir')}
				</Button>
			</div>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.localOneshotsDirLabel')}</span>
				<Input
					placeholder={t('settings.localOneshotsDirPlaceholder')}
					value={oneshots}
					onChange={(e) => {
						const v = e.currentTarget.value;
						setOneshots(v);
					}}
				/>
			</div>
			<p className="settings-local__hint">{t('settings.localOneshotsDirDesc')}</p>
			<div className="settings-kv__save">
				<Button
					variant="primary"
					disabled={!dirty}
					loading={save.isPending}
					onClick={() => void onSave()}
				>
					{t('common.save')}
				</Button>
			</div>
		</Section>
	);
}

function ServerSettingsSection() {
	const { t } = useTranslation();
	const query = useServerSettings();
	const save = useSaveServerSettings();
	const [rows, setRows] = useState<KvRow[] | null>(null);
	const [newKey, setNewKey] = useState('');
	const [newVal, setNewVal] = useState('');

	useEffect(() => {
		if (query.data && rows === null) {
			setRows(
				// localDirs is a read-only injected field and does not enter key-value editing (see the "Local source" section)
				Object.entries(query.data)
					.filter(([key]) => key !== 'localDirs')
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
		pushToast(t('settings.serverSettingsSaved'), 'info');
	}

	function addRow() {
		const key = newKey.trim();
		if (!key) return;
		setRows((cur) => [...(cur ?? []), { key, text: newVal, isNew: true }]);
		setNewKey('');
		setNewVal('');
	}

	return (
		<Section title={t('settings.serverSettingsTitle')} desc={t('settings.serverSettingsDesc')}>
			{rows === null ? (
				<Skeleton height={48} />
			) : (
				<>
					{rows.length === 0 ? (
						<p className="faint settings-kv__empty">{t('settings.noConfig')}</p>
					) : (
						<div className="settings-kv">
							{rows.map((row, i) => (
								<div key={i} className="settings-kv__row">
									<Input
										className={row.isNew ? '' : 'settings-kv__key'}
										placeholder={t('settings.keyPlaceholder')}
										value={row.key}
										readOnly={!row.isNew}
										onChange={(e) => {
											// currentTarget is already null by the time the deferred updater runs, so take the value first
											const v = e.currentTarget.value;
											setRows((cur) => (cur ?? []).map((r, j) => (j === i ? { ...r, key: v } : r)));
										}}
									/>
									<Input
										placeholder={t('settings.valuePlaceholder')}
										value={row.text}
										onChange={(e) => {
											const v = e.currentTarget.value;
											setRows((cur) => (cur ?? []).map((r, j) => (j === i ? { ...r, text: v } : r)));
										}}
									/>
									<IconButton
										label={t('common.delete')}
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
						<Input placeholder={t('settings.keyPlaceholder')} value={newKey} onChange={(e) => setNewKey(e.currentTarget.value)} />
						<Input
							placeholder={t('settings.valuePlaceholder')}
							value={newVal}
							onChange={(e) => setNewVal(e.currentTarget.value)}
						/>
						<Button disabled={!newKey.trim()} onClick={addRow}>
							{t('settings.add')}
						</Button>
					</div>
					<div className="settings-kv__save">
						<Button variant="primary" loading={save.isPending} onClick={() => void onSave()}>
							{t('common.save')}
						</Button>
					</div>
				</>
			)}
		</Section>
	);
}

const BACKUP_SECTIONS = [
	{ key: 'library', labelKey: 'settings.backupLibrary' },
	{ key: 'categories', labelKey: 'settings.backupCategories' },
	{ key: 'progress', labelKey: 'settings.backupProgress' },
	{ key: 'settings', labelKey: 'settings.backupSettings' },
	{ key: 'sourceSettings', labelKey: 'settings.backupSourceSettings' }
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
	const { t } = useTranslation();
	return (
		<div className="settings-backup__result">
			<p className="settings-modal-text">
				{t('settings.restored', {
					library: result.restored.library,
					categories: result.restored.categories,
					progress: result.restored.progress,
					settings: result.restored.settings,
					sourceSettings: result.restored.sourceSettings
				})}
			</p>
			{result.warnings.length === 0 ? (
				<p className="faint">{t('settings.restoredAll')}</p>
			) : (
				<>
					<p className="settings-modal-text">{t('settings.skippedItems')}</p>
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

function aidokuSkippedLabel(t: TFunction, s: AidokuImportSkipped): string {
	const what = s.section === 'library' ? t('settings.libraryEntry') : t('settings.progressEntry');
	const chapter = s.chapterKey ? t('settings.chapterSuffix', { key: s.chapterKey }) : '';
	return `${what} ${s.sourceKey}/${s.mangaKey}${chapter}`;
}

function AidokuImportResultView({ result }: { result: AidokuImportResult }) {
	const { t } = useTranslation();
	return (
		<div className="settings-backup__result">
			<p className="settings-modal-text">
				{t('settings.imported', {
					library: result.imported.library,
					categories: result.imported.categories,
					progress: result.imported.progress
				})}
			</p>
			{result.skipped.length === 0 ? (
				<p className="faint">{t('settings.importedAll')}</p>
			) : (
				<>
					<p className="settings-modal-text">{t('settings.skippedNoSource')}</p>
					<ul className="settings-backup__warnings">
						{result.skipped.map((s, i) => (
							<li key={i}>{aidokuSkippedLabel(t, s)}</li>
						))}
					</ul>
				</>
			)}
		</div>
	);
}

function suwayomiSkippedLabel(t: TFunction, s: SuwayomiImportSkipped): string {
	const what = s.section === 'library' ? t('settings.libraryEntry') : t('settings.progressEntry');
	const name = s.mihonSourceName || t('settings.sourceId', { id: s.mihonSourceId });
	return `${what} ${name}/${s.mangaKey}`;
}

function suwayomiGroupName(t: TFunction, g: SuwayomiSourceGroup): string {
	return g.mihonSourceName || t('settings.unknownSource', { id: g.mihonSourceId });
}

/** Mapping dropdown for an unmatched group: same-name candidates first, then the other installed sources; left empty the whole group is skipped */
function SuwayomiMappingSelect({
	group,
	value,
	onChange
}: {
	group: SuwayomiSourceGroup;
	value: string;
	onChange: (key: string) => void;
}) {
	const { t } = useTranslation();
	const sources = useSources();
	const candidateKeys = new Set(group.candidates.map((c) => c.key));
	const rest = (sources.data ?? []).filter((s) => !candidateKeys.has(s.key));
	return (
		<Select value={value} onChange={(e) => onChange(e.currentTarget.value)}>
			<option value="">{t('settings.skipSource')}</option>
			{group.candidates.map((c) => (
				<option key={c.key} value={c.key}>
					{t('settings.sourceOption', { name: c.name, key: c.key })}
				</option>
			))}
			{rest.map((s) => (
				<option key={s.key} value={s.key}>
					{t('settings.sourceOption', { name: s.name, key: s.key })}
				</option>
			))}
		</Select>
	);
}

function SuwayomiImportResultView({ result }: { result: SuwayomiImportResult }) {
	const { t } = useTranslation();
	return (
		<div className="settings-backup__result">
			<p className="settings-modal-text">
				{t('settings.imported', {
					library: result.imported.library,
					categories: result.imported.categories,
					progress: result.imported.progress
				})}
			</p>
			<p className="settings-modal-text">
				{result.sources
					.map((g) => {
						const label = suwayomiGroupName(t, g);
						if (g.match === 'unmatched') return t('settings.groupUnmapped', { name: label, count: g.mangaCount });
						const how = g.match === 'auto' ? t('settings.matchAuto') : t('settings.matchManual');
						return t('settings.groupMapped', {
							name: label,
							count: g.mangaCount,
							how,
							target: g.aidokuSourceName ?? g.aidokuSourceKey
						});
					})
					.join(t('settings.listSeparator'))}
			</p>
			{result.skipped.length > 0 && (
				<>
					<p className="settings-modal-text">{t('settings.skippedItems')}</p>
					<ul className="settings-backup__warnings">
						{result.skipped.map((s, i) => (
							<li key={i}>{suwayomiSkippedLabel(t, s)}</li>
						))}
					</ul>
				</>
			)}
		</div>
	);
}

function BackupSection({ isAdmin }: { isAdmin: boolean }) {
	const { t } = useTranslation();
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
	// The settings/sourceSettings sections write global state; the server rejects them for readers (403), so the options are hidden too
	const sections = isAdmin
		? BACKUP_SECTIONS
		: BACKUP_SECTIONS.filter((s) => s.key === 'library' || s.key === 'categories' || s.key === 'progress');

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
			// api() already popped an error toast
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
			// api() already popped an error toast
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
			// api() already popped an error toast
		}
	}

	async function confirmAibImport() {
		if (!aib) return;
		try {
			const res = await importAidoku.mutateAsync({ file: aib.file, dryRun: false });
			setAibResult(res);
		} catch {
			// api() already popped an error toast
		}
	}

	function closeAibModal() {
		setAib(null);
		setAibResult(null);
	}

	async function onFile(e: React.ChangeEvent<HTMLInputElement>) {
		const file = e.currentTarget.files?.[0];
		// Clear value so picking the same file again still fires change
		e.currentTarget.value = '';
		if (!file) return;
		let doc: unknown;
		try {
			doc = JSON.parse(await file.text());
		} catch {
			pushToast(t('settings.invalidBackupJson'));
			return;
		}
		const version = (doc as { version?: unknown } | null)?.version;
		if (version !== 1) {
			pushToast(t('settings.unsupportedBackupVersion', { version: typeof version === 'number' ? version : t('settings.versionMissing') }));
			return;
		}
		const createdAt = (doc as { createdAt?: unknown }).createdAt;
		setPreview({
			doc,
			createdAt: typeof createdAt === 'number' ? createdAt : null,
			counts: previewCounts(doc as Record<string, unknown>)
		});
		setPicked(sections.map((s) => s.key));
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
			// api() already popped an error toast
		}
	}

	function closeModal() {
		setPreview(null);
		setResult(null);
	}

	return (
		<Section
			title={t('settings.backupTitle')}
			desc={t('settings.backupDesc')}
		>
			<div className="settings-backup__ops">
				<a className="btn settings-backup__download" href={backupUrl()} download>
					<Download size={15} />
					{t('settings.downloadBackup')}
				</a>
				<Button onClick={() => fileRef.current?.click()}>
					<Upload size={15} />
					{t('settings.uploadBackup')}
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
					{t('settings.importAidoku')}
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
					{t('settings.importSuwayomi')}
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
				title="{t('settings.importSuwayomi')}"
				onClose={closeSwModal}
				footer={
					swResult ? (
						<Button variant="primary" onClick={closeSwModal}>
							{t('settings.done')}
						</Button>
					) : (
						<>
							<Button onClick={closeSwModal}>{t('common.cancel')}</Button>
							<Button variant="primary" loading={importSuwayomi.isPending} onClick={() => void confirmSwImport()}>
								{t('settings.confirmImport')}
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
								{t('settings.willImport', {
									library: sw.preview.imported.library,
									categories: sw.preview.imported.categories,
									progress: sw.preview.imported.progress
								})}
							</p>
							<p className="settings-modal-text">{t('settings.sourceMapping')}</p>
							<ul className="settings-backup__mapping">
								{sw.preview.sources.map((g) => (
									<li key={g.mihonSourceId} className="settings-backup__mapping-row">
										<span className="settings-backup__mapping-name">
											{t('settings.groupWithCount', { name: suwayomiGroupName(t, g), count: g.mangaCount })}
										</span>
										{g.match === 'unmatched' ? (
											<SuwayomiMappingSelect
												group={g}
												value={swMapping[g.mihonSourceId] ?? ''}
												onChange={(key) => setSwSource(g.mihonSourceId, key)}
											/>
										) : (
											<span className="faint">{t('settings.autoMatched', { name: g.aidokuSourceName ?? g.aidokuSourceKey })}</span>
										)}
									</li>
								))}
							</ul>
							{sw.preview.skipped.length > 0 && (
								<p className="settings-modal-text">
									{t('settings.unmappedSkipped', { count: sw.preview.skipped.length })}
								</p>
							)}
							<p className="settings-backup__warn">
								{t('settings.importMergeWarnSuwayomi')}
							</p>
						</>
					)
				)}
			</Modal>

			<Modal
				open={aib !== null}
				title="{t('settings.importAidoku')}"
				onClose={closeAibModal}
				footer={
					aibResult ? (
						<Button variant="primary" onClick={closeAibModal}>
							{t('settings.done')}
						</Button>
					) : (
						<>
							<Button onClick={closeAibModal}>{t('common.cancel')}</Button>
							<Button variant="primary" loading={importAidoku.isPending} onClick={() => void confirmAibImport()}>
								{t('settings.confirmImport')}
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
								{t('settings.willImport', {
									library: aib.preview.imported.library,
									categories: aib.preview.imported.categories,
									progress: aib.preview.imported.progress
								})}
							</p>
							{aib.preview.skipped.length > 0 && (
								<>
									<p className="settings-modal-text">{t('settings.willSkipNoSource')}</p>
									<ul className="settings-backup__warnings">
										{aib.preview.skipped.map((s, i) => (
											<li key={i}>{aidokuSkippedLabel(t, s)}</li>
										))}
									</ul>
								</>
							)}
							<p className="settings-backup__warn">
								{t('settings.importMergeWarn')}
							</p>
						</>
					)
				)}
			</Modal>

			<Modal
				open={preview !== null}
				title={t('settings.restoreBackup')}
				onClose={closeModal}
				footer={
					result ? (
						<Button variant="primary" onClick={closeModal}>
							{t('settings.done')}
						</Button>
					) : (
						<>
							<Button onClick={closeModal}>{t('common.cancel')}</Button>
							<Button
								variant="danger"
								loading={restore.isPending}
								disabled={picked.length === 0}
								onClick={() => void confirmRestore()}
							>
								{t('settings.import')}
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
									? t('settings.restorePickWithDate', { date: new Date(preview.createdAt * 1000).toLocaleString() })
									: t('settings.restorePick')}
							</p>
							<div className="settings-backup__sections">
								{sections.map((s) => (
									<label key={s.key} className="settings-backup__section">
										<input
											type="checkbox"
											checked={picked.includes(s.key)}
											onChange={(e) => toggle(s.key, e.currentTarget.checked)}
										/>
										{t('settings.sectionWithCount', { name: t(s.labelKey), count: preview.counts[s.key] ?? 0 })}
									</label>
								))}
							</div>
							<p className="settings-backup__warn">
								{t('settings.restoreWarn')}
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

function DownloadMoveSection() {
	const { t } = useTranslation();
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const [text, setText] = useState<string | null>(null);

	useEffect(() => {
		if (!settings.data) return;
		if (text === null) setText(String(settings.data['download.move_path'] ?? ''));
	}, [settings.data, text]);

	const data = settings.data;
	if (!data || text === null) {
		return (
			<Section
				title={t('settings.downloadMoveTitle')}
				desc={t('settings.downloadMoveDescShort')}
			>
				<Skeleton height={48} />
			</Section>
		);
	}

	const current = String(data['download.move_path'] ?? '');

	function commit() {
		if (text === null) return;
		const value = text.trim();
		if (value !== current) save.mutate({ 'download.move_path': value });
	}

	return (
		<Section
			title={t('settings.downloadMoveTitle')}
			desc={t('settings.downloadMoveDesc')}
		>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.moveTemplate')}</span>
				<Input
					placeholder="/mnt/library/{author}/{manga}/{filename}"
					value={text}
					disabled={save.isPending}
					onChange={(e) => setText(e.currentTarget.value)}
					onBlur={commit}
					onKeyDown={(e) => {
						if (e.key === 'Enter') commit();
					}}
				/>
			</div>
			<p className="faint settings-backup__hint">
				{t('settings.moveTemplateHint')}
			</p>
		</Section>
	);
}

function ScheduledBackupSection() {
	const { t } = useTranslation();
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
				title={t('settings.autoBackupTitle')}
				desc={t('settings.autoBackupDesc')}
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
			pushToast(t('settings.minInteger', { min }));
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
			// api() already popped an error toast
		}
	}

	function closeModal() {
		setConfirming(null);
		setResult(null);
	}

	return (
		<Section
			title={t('settings.autoBackupTitle')}
			desc={t('settings.autoBackupDesc')}
		>
			<label className="settings-backup__section">
				<input
					type="checkbox"
					checked={enabled}
					onChange={(e) => save.mutate({ 'backup.enabled': e.currentTarget.checked })}
				/>
				{t('settings.enableAutoBackup')}
			</label>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.backupInterval')}</span>
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
				<span className="settings-row__label">{t('settings.retentionCount')}</span>
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
				<span className="settings-row__label">{t('settings.backupDir')}</span>
				<span className="settings-backup__dir">{backups.data?.dir ?? '…'}</span>
			</div>
			<p className="faint settings-backup__hint">
				{t('settings.backupDirHint', {
					lastRun: typeof lastRun === 'number' ? new Date(lastRun * 1000).toLocaleString() : t('settings.never')
				})}
			</p>

			<div className="settings-backup__files-head">
				<span className="settings-row__label">{t('settings.existingBackups')}</span>
				<IconButton label={t('settings.refresh')} size="sm" onClick={() => void backups.refetch()}>
					<RotateCw size={14} />
				</IconButton>
			</div>
			{files.length === 0 ? (
				<p className="faint">{t('settings.noBackups')}</p>
			) : (
				<div className="settings-backup__files">
					{files.map((f) => (
						<div key={f.name} className="settings-backup__file">
							<div className="settings-backup__file-meta">
								<span>{new Date(f.createdAt * 1000).toLocaleString()}</span>
								<span className="faint">{formatSize(f.size)}</span>
							</div>
							<Button size="sm" onClick={() => setConfirming(f)}>
								{t('settings.restore')}
							</Button>
						</div>
					))}
				</div>
			)}

			<Modal
				open={confirming !== null || result !== null}
				title={t('settings.restoreBackup')}
				onClose={closeModal}
				footer={
					result ? (
						<Button variant="primary" onClick={closeModal}>
							{t('settings.done')}
						</Button>
					) : (
						<>
							<Button onClick={closeModal}>{t('common.cancel')}</Button>
							<Button variant="danger" loading={restore.isPending} onClick={() => void confirmRestore()}>
								{t('settings.restore')}
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
								{t('settings.restoreNamedConfirm', { date: new Date(confirming.createdAt * 1000).toLocaleString() })}
							</p>
							<p className="settings-backup__warn">
								{t('settings.backupTrustWarn')}
							</p>
						</>
					)
				)}
			</Modal>
		</Section>
	);
}

function UsersSection() {
	const { t } = useTranslation();
	const navigate = useNavigate();
	return (
		<Section title={t('settings.usersTitle')} desc={t('settings.usersDesc')}>
			<div className="settings-row">
				<span className="settings-row__label">{t('settings.accountsRoles')}</span>
				<Button onClick={() => navigate('/users')}>{t('settings.manageUsers')}</Button>
			</div>
		</Section>
	);
}

function AboutSection() {
	const { t } = useTranslation();
	const [version, setVersion] = useState<string | null>(null);
	const [latest, setLatest] = useState<{ tag: string; url: string } | null>(null);
	useEffect(() => {
		let cancelled = false;
		(async () => {
			let v: string;
			try {
				v = (await api<{ version: string }>('/version', { silent: true })).version;
			} catch {
				return;
			}
			if (cancelled) return;
			setVersion(v);
			try {
				const res = await fetch('https://api.github.com/repos/Sumidoku/Sumidoku/releases/latest');
				if (!res.ok) return;
				const rel = (await res.json()) as { tag_name?: string; html_url?: string };
				if (!cancelled && rel.tag_name && rel.html_url) {
					setLatest({ tag: rel.tag_name, url: rel.html_url });
				}
			} catch {
				// Offline/rate-limiting is normal: degrade silently and show only the local version
			}
		})();
		return () => {
			cancelled = true;
		};
	}, []);
	const newer = version && latest ? compareVersions(latest.tag, version) : null;
	const update = newer !== null && newer > 0 ? latest : null;
	const upToDate = newer !== null && newer <= 0;
	return (
		<Section title={t('settings.aboutTitle')} desc={t('settings.aboutDesc')}>
			<div className="settings-about">
				<p>
					<strong>Sumidoku</strong> · {t('settings.tagline')}
				</p>
				<p className="muted">{t('settings.endpoints')}</p>
				{version && (
					<p className="muted">
						{t('settings.currentVersion', { version })}
						{update && (
							<>
								{' · '}
								<a className="setting-link" href={update.url} target="_blank" rel="noreferrer">
									{t('settings.newVersion', { tag: update.tag })}
									<ExternalLink size={13} />
								</a>
							</>
						)}
						{upToDate && ` · ${t('settings.upToDate')}`}
					</p>
				)}
				<p className="faint">
					{t('settings.opdsHint')}
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
	const { t } = useTranslation();
	// The server's 403 is the real gate; here we just keep readers from seeing entries that are doomed to fail
	const isAdmin = useIsAdmin();
	return (
		<div className="page container settings-page">
			<PageHeader title={t('common.nav.settings')} />
			<AppearanceSection />
			<IncognitoSection />
			<SecuritySection />
			{isAdmin && <UsersSection />}
			<CategoriesSection />
			<FilterGroupsSection />
			<LibraryDisplaySection />
			{isAdmin && <LibraryUpdateSection />}
			{isAdmin && <LocalSourceSection />}
			{isAdmin && <ServerSettingsSection />}
			<BackupSection isAdmin={isAdmin} />
			{isAdmin && <DownloadMoveSection />}
			{isAdmin && <ScheduledBackupSection />}
			<AboutSection />
		</div>
	);
}
