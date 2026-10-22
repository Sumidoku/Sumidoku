import { useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
	ArrowLeftRight,
	CircleArrowUp,
	GripVertical,
	House,
	PackageOpen,
	Pause,
	Pin,
	PinOff,
	Play,
	Plus,
	ScanSearch,
	Trash,
	TriangleAlert,
	Upload
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ApiError, sourceIconUrl } from '../api/client';
import {
	useDeleteSource,
	useImportSource,
	usePinSource,
	useRegisterOpdsSource,
	useReorderPinned,
	useScanSources,
	useSetSourceEnabled,
	useSources,
	useUpdateAllSources,
	useUpdateSource
} from '../api/hooks';
import type { Source } from '../api/types';
import { translateApiError } from '../i18n';
import { compareSourceName, contentRatingColor, contentRatingLabel, languageLabel } from '../lib/source';
import {
	Button,
	Chip,
	EmptyState,
	IconButton,
	Input,
	Modal,
	PageHeader,
	SegmentedControl,
	Skeleton,
	pushToast
} from '../components';
import ReposPanel from './browse/ReposPanel';
import { SourceSettingsButton } from './browse/SourceSettingsModal';
import { iconColor } from './browse/iconColor';

function SourceIcon({ source }: { source: Source }) {
	const [failed, setFailed] = useState(false);
	if (failed) {
		return (
			<span className="source-card__icon-fallback" style={{ background: iconColor(source.key) }}>
				{source.name.slice(0, 1)}
			</span>
		);
	}
	return (
		<span className="source-card__icon">
			<img
				src={sourceIconUrl(source.id)}
				alt=""
				loading="lazy"
				referrerPolicy="no-referrer"
				onError={() => setFailed(true)}
			/>
		</span>
	);
}

function UpdateButton({ source }: { source: Source }) {
	const { t } = useTranslation();
	const update = useUpdateSource();
	const navigate = useNavigate();

	async function onClick(e: React.MouseEvent) {
		e.stopPropagation();
		try {
			const r = await update.mutateAsync(source.id);
			pushToast(t('browse.updated', { name: source.name, version: r.version }), 'info');
			for (const w of r.migrationWarnings.slice(0, 3)) pushToast(w, 'info');
			if (r.migrationRequired) {
				pushToast(t('browse.migrationRequired', { name: source.name }), 'info');
				navigate(`/migrate?source=${source.id}&sameSource=1`);
			}
		} catch {
			// client already popped an error toast
		}
	}

	return (
		<Button size="sm" variant="primary" loading={update.isPending} onClick={(e) => void onClick(e)}>
			<CircleArrowUp size={14} />
			v{source.updateVersion}
		</Button>
	);
}

function ImportAixButton() {
	const { t } = useTranslation();
	const importSource = useImportSource();
	const navigate = useNavigate();
	const inputRef = useRef<HTMLInputElement>(null);

	async function onFile(e: React.ChangeEvent<HTMLInputElement>) {
		const file = e.currentTarget.files?.[0];
		// reset eagerly: re-picking the same file must fire change again, and currentTarget dies with the dispatch
		e.currentTarget.value = '';
		if (!file) return;
		try {
			const r = await importSource.mutateAsync(file);
			pushToast(t('browse.imported', { name: r.name, version: r.version }), 'info');
			for (const w of r.migrationWarnings.slice(0, 3)) pushToast(w, 'info');
			if (r.migrationRequired) {
				pushToast(t('browse.migrationRequired', { name: r.name }), 'info');
				navigate(`/migrate?source=${r.id}&sameSource=1`);
			}
		} catch {
			// client already popped an error toast
		}
	}

	return (
		<>
			<input ref={inputRef} type="file" accept=".aix" hidden onChange={(e) => void onFile(e)} />
			<Button loading={importSource.isPending} onClick={() => inputRef.current?.click()}>
				<Upload size={15} />
				{t('browse.importAix')}
			</Button>
		</>
	);
}

function OpdsRegisterModal({ open, onClose }: { open: boolean; onClose: () => void }) {
	const { t } = useTranslation();
	const register = useRegisterOpdsSource();
	const [name, setName] = useState('');
	const [url, setUrl] = useState('');
	const [username, setUsername] = useState('');
	const [password, setPassword] = useState('');

	function reset() {
		setName('');
		setUrl('');
		setUsername('');
		setPassword('');
	}

	async function submit() {
		try {
			const trimmedUrl = url.trim();
			const source = await register.mutateAsync({
				name: name.trim(),
				url: trimmedUrl,
				...(username.trim() ? { username: username.trim(), password } : {})
			});
			pushToast(t('browse.opdsAdded', { name: source.name }), 'info');
			reset();
			onClose();
		} catch (e) {
			if (e instanceof ApiError && e.status === 400) pushToast(translateApiError(e.code, e.message));
			else pushToast(t('browse.addFailed'));
		}
	}

	return (
		<Modal
			open={open}
			title={t('browse.addOpds')}
			onClose={onClose}
			footer={
				<>
					<Button onClick={onClose}>{t('common.cancel')}</Button>
					<Button
						variant="primary"
						loading={register.isPending}
						disabled={!name.trim() || !url.trim()}
						onClick={() => void submit()}
					>
						{t('browse.add')}
					</Button>
				</>
			}
		>
			<div className="opds-form">
				<Input
					placeholder={t('browse.opdsNamePlaceholder')}
					value={name}
					onChange={(e) => setName(e.currentTarget.value)}
				/>
				<Input
					placeholder={t('browse.opdsUrlPlaceholder')}
					value={url}
					onChange={(e) => setUrl(e.currentTarget.value)}
				/>
				<Input
					placeholder={t('browse.opdsUsernamePlaceholder')}
					autoComplete="off"
					value={username}
					onChange={(e) => setUsername(e.currentTarget.value)}
				/>
				<Input
					type="password"
					placeholder={t('browse.opdsPasswordPlaceholder')}
					autoComplete="new-password"
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				<p className="muted">{t('browse.opdsHint')}</p>
			</div>
		</Modal>
	);
}

/** Passed when the pinned section's cards enable drag-sorting; the plain list does not pass it */
interface CardDnd {
	dragging: boolean;
	dragOver: boolean;
	onDragStart: (e: React.DragEvent) => void;
	onDragOver: (e: React.DragEvent) => void;
	onDrop: () => void;
	onDragEnd: () => void;
}

function SourceCard({
	source: s,
	dnd,
	onTogglePin,
	onToggleEnabled,
	onDelete
}: {
	source: Source;
	dnd?: CardDnd;
	onTogglePin: (s: Source) => void;
	onToggleEnabled: (s: Source) => void;
	onDelete: (s: Source) => void;
}) {
	const { t } = useTranslation();
	const navigate = useNavigate();
	const cls = ['source-card'];
	if (s.enabled) cls.push('source-card--link');
	else cls.push('source-card--disabled');
	if (dnd?.dragging) cls.push('source-card--dragging');
	if (dnd?.dragOver) cls.push('source-card--drag-over');
	return (
		<div
			className={cls.join(' ')}
			role={s.enabled ? 'link' : undefined}
			tabIndex={s.enabled ? 0 : undefined}
			onClick={() => s.enabled && navigate(`/browse/${s.id}`)}
			onKeyDown={(e) => {
				if (e.target !== e.currentTarget) return;
				if (s.enabled && (e.key === 'Enter' || e.key === ' ')) {
					e.preventDefault();
					navigate(`/browse/${s.id}`);
				}
			}}
			onDragOver={dnd?.onDragOver}
			onDrop={dnd?.onDrop}
		>
			{dnd && (
				// Only the handle is draggable: making the whole card draggable would turn click-to-navigate into a drag start
				<span
					className="source-card__grip"
					draggable
					onDragStart={dnd.onDragStart}
					onDragEnd={dnd.onDragEnd}
				>
					<GripVertical size={16} />
				</span>
			)}
			<SourceIcon source={s} />
			<div className="source-card__main">
				<div className="source-card__name">
					<span className="source-card__name-text">{s.name}</span>
					{s.contentRating > 0 && (
						<Chip color={contentRatingColor(s.contentRating)}>
							{contentRatingLabel(s.contentRating)}
						</Chip>
					)}
				</div>
				<div className="source-card__meta">
					{s.key} · v{s.version}
					{!s.enabled ? t('browse.disabled') : ''}
					{s.missingFromRepos && (
						<span
							className="source-card__missing"
							title={t('browse.missingFromRepos')}
						>
							<TriangleAlert size={13} />
						</span>
					)}
				</div>
				{s.languages.length > 0 && (
					<div className="source-card__badges">
						{s.languages.slice(0, 3).map((lang) => (
							<Chip key={lang}>{languageLabel(lang)}</Chip>
						))}
						{s.languages.length > 3 && <Chip>+{s.languages.length - 3}</Chip>}
					</div>
				)}
			</div>
			{s.updateVersion !== null && <UpdateButton source={s} />}
			{/* The built-in local source is always pinned at the top, so it gets no pin toggle of its own */}
			{s.kind !== 'local' && (
				<IconButton
					label={s.pinned ? t('browse.unpin', { name: s.name }) : t('browse.pin', { name: s.name })}
					onClick={(e) => {
						e.stopPropagation();
						onTogglePin(s);
					}}
				>
					{s.pinned ? <PinOff size={17} /> : <Pin size={17} />}
				</IconButton>
			)}
			{s.kind !== 'local' && (
				<IconButton
					label={s.enabled ? t('browse.disable', { name: s.name }) : t('browse.enable', { name: s.name })}
					onClick={(e) => {
						e.stopPropagation();
						onToggleEnabled(s);
					}}
				>
					{s.enabled ? <Pause size={17} /> : <Play size={17} />}
				</IconButton>
			)}
			{/* Built-in sources (local/opds) have no wasm capabilities (home/settings 404 — the local source has no home yet,
				so no home button either); the local source cannot be uninstalled (server 400) or unpinned, opds instances can be uninstalled */}
			{s.kind !== 'local' && (
				<>
					{s.kind !== 'opds' && s.hasHome !== false && (
						<IconButton
							label={t('browse.home', { name: s.name })}
							onClick={(e) => {
								e.stopPropagation();
								navigate(`/browse/${s.id}/home`);
							}}
						>
							<House size={17} />
						</IconButton>
					)}
					{s.kind !== 'opds' && <SourceSettingsButton sourceId={s.id} sourceName={s.name} />}
					<IconButton
						label={t('browse.uninstall', { name: s.name })}
						variant="danger"
						onClick={(e) => {
							e.stopPropagation();
							onDelete(s);
						}}
					>
						<Trash size={17} />
					</IconButton>
				</>
			)}
		</div>
	);
}

export default function BrowsePage() {
	const { t } = useTranslation();
	const sources = useSources();
	const scan = useScanSources();
	const del = useDeleteSource();
	const updateAll = useUpdateAllSources();
	const pin = usePinSource();
	const setEnabled = useSetSourceEnabled();
	const reorderPinned = useReorderPinned();
	const navigate = useNavigate();
	const [pendingDelete, setPendingDelete] = useState<Source | null>(null);
	const [registeringOpds, setRegisteringOpds] = useState(false);
	const [tab, setTab] = useState<'installed' | 'repos'>('installed');
	const [dragIndex, setDragIndex] = useState<number | null>(null);
	const [overIndex, setOverIndex] = useState<number | null>(null);

	// Installed sources sort alphabetically by name; the pinned section sorts separately by pinOrder.
	// The built-in local source has no pin state of its own: it is always shown pinned at the top
	const all = useMemo(() => [...(sources.data ?? [])].sort(compareSourceName), [sources.data]);
	// Aidoku-style separate section: updatable sources grouped together, below the pinned section and above the full list
	const updateSources = all.filter((s) => s.updateVersion !== null);
	const updateCount = updateSources.length;
	const pinnedSources = all
		.filter((s) => s.pinned || s.kind === 'local')
		.sort((a, b) => {
			if (a.kind === 'local') return -1;
			if (b.kind === 'local') return 1;
			return (a.pinOrder ?? 0) - (b.pinOrder ?? 0);
		});

	async function onScan() {
		try {
			const r = await scan.mutateAsync();
			pushToast(
				t('browse.scanDone', {
					installed: r.installed.length,
					updated: r.updated.length,
					skipped: r.skipped.length
				}),
				'info'
			);
			for (const s of r.skipped.slice(0, 3)) {
				pushToast(t('browse.scanSkipped', { dir: s.dir, reason: s.reason }), 'info');
			}
		} catch {
			// client already popped an error toast
		}
	}

	async function onUpdateAll() {
		try {
			const r = await updateAll.mutateAsync();
			pushToast(
				t('browse.updateAllDone', { updated: r.updated.length, failed: r.failed.length }),
				'info'
			);
			for (const f of r.failed.slice(0, 3)) {
				pushToast(t('browse.updateFailed', { key: f.key, reason: f.reason }), 'info');
			}
			for (const w of r.migrationWarnings.slice(0, 3)) {
				pushToast(t('browse.migrationWarning', { key: w.key, message: w.message }), 'info');
			}
			if (r.migrationRequired.length > 0) {
				pushToast(
					t('browse.migrationRequiredMany', {
						names: r.migrationRequired.join(t('browse.listSeparator'))
					}),
					'info'
				);
				navigate('/migrate');
			}
		} catch {
			// client already popped an error toast
		}
	}

	async function togglePin(s: Source) {
		try {
			await pin.mutateAsync({ id: s.id, pinned: !s.pinned });
			pushToast(
				s.pinned ? t('browse.unpinned', { name: s.name }) : t('browse.pinned', { name: s.name }),
				'info'
			);
		} catch {
			// client already popped an error toast
		}
	}

	async function toggleEnabled(s: Source) {
		try {
			await setEnabled.mutateAsync({ id: s.id, enabled: !s.enabled });
			pushToast(
				s.enabled ? t('browse.disabledToast', { name: s.name }) : t('browse.enabledToast', { name: s.name }),
				'info'
			);
		} catch {
			// client already popped an error toast
		}
	}

	function onDropPinned(target: number) {
		const from = dragIndex;
		setDragIndex(null);
		setOverIndex(null);
		if (from === null || from === target) return;
		// The local source occupies index 0 of the rendered section but is not part of the
		// server-side pinned set, so drag indices shift by one and its key stays out of the payload
		const keys = pinnedSources.filter((s) => s.kind !== 'local').map((s) => s.key);
		const [moved] = keys.splice(from - 1, 1);
		keys.splice(target - 1, 0, moved);
		reorderPinned.mutate(keys);
	}

	async function confirmDelete() {
		if (!pendingDelete) return;
		try {
			await del.mutateAsync(pendingDelete.id);
			pushToast(t('browse.uninstalled', { name: pendingDelete.name }), 'info');
			setPendingDelete(null);
		} catch {
			// client already popped an error toast
		}
	}

	return (
		<div className="page container">
			<PageHeader
				title={t('common.nav.browse')}
				actions={
					tab === 'installed' ? (
						<>
							<Button onClick={() => navigate('/migrate')}>
								<ArrowLeftRight size={15} />
								{t('browse.migrate')}
							</Button>
							{updateCount > 0 && (
								<Button loading={updateAll.isPending} onClick={() => void onUpdateAll()}>
									<CircleArrowUp size={15} />
									{t('browse.updateAll', { count: updateCount })}
								</Button>
							)}
							<Button onClick={() => setRegisteringOpds(true)}>
								<Plus size={15} />
								{t('browse.addOpds')}
							</Button>
							<ImportAixButton />
							<Button variant="primary" loading={scan.isPending} onClick={() => void onScan()}>
								<ScanSearch size={15} />
								{t('browse.scan')}
							</Button>
						</>
					) : undefined
				}
			/>

			<div className="browse__tabs">
				<SegmentedControl
					options={[
						{ value: 'installed', label: t('browse.tabInstalled') },
						{ value: 'repos', label: t('browse.tabRepos') }
					]}
					value={tab}
					onChange={setTab}
				/>
			</div>

			{tab === 'repos' ? (
				<ReposPanel />
			) : sources.isPending ? (
				<div className="source-list">
					{Array.from({ length: 4 }, (_, i) => (
						<Skeleton key={i} height={82} style={{ borderRadius: 'var(--r-md)' }} />
					))}
				</div>
			) : all.length === 0 ? (
				<EmptyState
					icon={PackageOpen}
					title={t('browse.emptyTitle')}
					hint={t('browse.emptyHintAdmin')}
				/>
			) : (
				<>
					{pinnedSources.length > 0 && (
						<>
							<h2 className="browse__section">{t('browse.sectionPinned')}</h2>
							<div className="source-list">
								{pinnedSources.map((s, i) => (
									<SourceCard
										key={s.id}
										source={s}
										// The local source is a fixed header of the section: no grip, no drag reordering
										dnd={
											s.kind !== 'local'
												? {
														dragging: dragIndex === i,
														dragOver: overIndex === i && dragIndex !== null && dragIndex !== i,
														onDragStart: (e) => {
															e.dataTransfer.setData('text/plain', s.key);
															e.dataTransfer.effectAllowed = 'move';
															setDragIndex(i);
														},
														onDragOver: (e) => {
															e.preventDefault();
															setOverIndex(i);
														},
														onDrop: () => onDropPinned(i),
														onDragEnd: () => {
															setDragIndex(null);
															setOverIndex(null);
														}
													}
												: undefined
										}
										onTogglePin={(s) => void togglePin(s)}
										onToggleEnabled={(s) => void toggleEnabled(s)}
										onDelete={setPendingDelete}
									/>
								))}
							</div>
						</>
					)}
					{updateSources.length > 0 && (
						<>
							<h2 className="browse__section">
								{t('browse.sectionUpdates', { count: updateSources.length })}
							</h2>
							<div className="source-list">
								{updateSources.map((s) => (
									<SourceCard
										key={s.id}
										source={s}
										onTogglePin={(s) => void togglePin(s)}
										onToggleEnabled={(s) => void toggleEnabled(s)}
										onDelete={setPendingDelete}
									/>
								))}
							</div>
						</>
					)}
					{(pinnedSources.length > 0 || updateSources.length > 0) && (
						// when any section appears above, the full list needs an "All" heading to set it apart
						<h2 className="browse__section">{t('browse.sectionAll')}</h2>
					)}
					<div className="source-list">
						{all.map((s) => (
							<SourceCard
								key={s.id}
								source={s}
								onTogglePin={(s) => void togglePin(s)}
								onToggleEnabled={(s) => void toggleEnabled(s)}
								onDelete={setPendingDelete}
							/>
						))}
					</div>
				</>
			)}

			<Modal
				open={pendingDelete !== null}
				title={t('browse.uninstallTitle', { name: pendingDelete?.name ?? '' })}
				onClose={() => setPendingDelete(null)}
				footer={
					<>
						<Button onClick={() => setPendingDelete(null)}>{t('common.cancel')}</Button>
						<Button variant="danger" loading={del.isPending} onClick={() => void confirmDelete()}>
							{t('browse.uninstallConfirm')}
						</Button>
					</>
				}
			>
				<p className="muted">{t('browse.deleteHint')}</p>
			</Modal>
			<OpdsRegisterModal open={registeringOpds} onClose={() => setRegisteringOpds(false)} />
		</div>
	);
}
