import { useMemo, useRef, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
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
	Settings,
	Trash,
	TriangleAlert,
	Upload
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { sourceIconUrl } from '../api/client';
import {
	useDeleteSource,
	useImportSource,
	usePinSource,
	useReorderPinned,
	useScanSources,
	useSetSourceEnabled,
	useSources,
	useUpdateAllSources,
	useUpdateSource
} from '../api/hooks';
import type { Source } from '../api/types';
import { compareSourceName, contentRatingColor, contentRatingLabel, languageLabel } from '../lib/source';
import {
	Button,
	Chip,
	EmptyState,
	IconButton,
	MenuButton,
	Modal,
	PageHeader,
	SegmentedControl,
	Skeleton,
	pushToast
} from '../components';
import ReposPanel from './browse/ReposPanel';
import { SearchPanel } from './browse/SearchPanel';
import { InternalSourceModal } from './browse/InternalSourceModal';
import { SourceSettingsButton, SourceSettingsDialog } from './browse/SourceSettingsModal';
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

function useSourceUpdate(source: Source) {
	const { t } = useTranslation();
	const update = useUpdateSource();
	const navigate = useNavigate();

	async function onUpdate() {
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

	return { onUpdate, pending: update.isPending };
}

function UpdateButton({ source, onUpdate, pending }: { source: Source; onUpdate: () => Promise<void>; pending: boolean }) {
	return (
		<Button
			size="sm"
			variant="primary"
			loading={pending}
			onClick={(e) => {
				e.stopPropagation();
				void onUpdate();
			}}
		>
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

/** Passed when the pinned section's cards enable drag-sorting; the plain list does not pass it */
interface CardDnd {
	dragging: boolean;
	dragOver: boolean;
	onDragStart: (e: React.DragEvent) => void;
	onDragOver: (e: React.DragEvent) => void;
	onDrop: () => void;
	onDragEnd: () => void;
}

export function SourceCard({
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
	const update = useSourceUpdate(s);
	const [settingsOpen, setSettingsOpen] = useState(false);
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
			<div className="source-card__actions">
				{s.updateVersion !== null && <UpdateButton source={s} onUpdate={update.onUpdate} pending={update.pending} />}
				<IconButton
					label={s.pinned ? t('browse.unpin', { name: s.name }) : t('browse.pin', { name: s.name })}
					onClick={(e) => {
						e.stopPropagation();
						onTogglePin(s);
					}}
				>
					{s.pinned ? <PinOff size={17} /> : <Pin size={17} />}
				</IconButton>
				{/* Disabling only gates wasm/opds instantiation today; the local scan reads the filesystem directly */}
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
				{/* Internal sources (local/opds) have no wasm home; local and aidoku sources have settings,
					opds credentials are write-once at registration; every source can be uninstalled */}
				{s.kind !== 'local' && s.kind !== 'opds' && s.hasHome !== false && (
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
				{s.kind !== 'opds' && <SourceSettingsButton sourceId={s.id} sourceName={s.name} sourceKind={s.kind} />}
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
			</div>
			<div className="source-card__mobile-actions">
				<MenuButton label={t('browse.actions', { name: s.name })}>
					{(close) => (
						<>
							{s.updateVersion !== null && (
								<button type="button" className="menu-row" disabled={update.pending} onClick={() => { close(); void update.onUpdate(); }}>
									<span>{t('browse.updateAction', { version: s.updateVersion })}</span>
									<CircleArrowUp size={14} />
								</button>
							)}
							<button type="button" className="menu-row" onClick={() => { close(); onTogglePin(s); }}>
								<span>{s.pinned ? t('browse.unpinAction') : t('browse.pinAction')}</span>
								{s.pinned ? <PinOff size={14} /> : <Pin size={14} />}
							</button>
							{s.kind !== 'local' && (
								<button type="button" className="menu-row" onClick={() => { close(); onToggleEnabled(s); }}>
									<span>{s.enabled ? t('browse.disableAction') : t('browse.enableAction')}</span>
									{s.enabled ? <Pause size={14} /> : <Play size={14} />}
								</button>
							)}
							{s.kind !== 'local' && s.kind !== 'opds' && s.hasHome !== false && (
								<button type="button" className="menu-row" onClick={() => { close(); navigate(`/browse/${s.id}/home`); }}>
									<span>{t('browse.homeAction')}</span>
									<House size={14} />
								</button>
							)}
							{s.kind !== 'opds' && (
								<button type="button" className="menu-row" onClick={() => { close(); setSettingsOpen(true); }}>
									<span>{t('common.nav.settings')}</span>
									<Settings size={14} />
								</button>
							)}
							<button type="button" className="menu-row menu-row--danger" onClick={() => { close(); onDelete(s); }}>
								<span>{t('browse.uninstallConfirm')}</span>
								<Trash size={14} />
							</button>
						</>
					)}
				</MenuButton>
			</div>
			<SourceSettingsDialog sourceId={s.id} sourceName={s.name} sourceKind={s.kind} open={settingsOpen} onClose={() => setSettingsOpen(false)} />
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
	const [searchParams, setSearchParams] = useSearchParams();
	const searching = searchParams.get('view') === 'search';
	const [pendingDelete, setPendingDelete] = useState<Source | null>(null);
	const [registeringInternal, setRegisteringInternal] = useState(false);
	const [tab, setTab] = useState<'installed' | 'repos'>('installed');
	const [dragIndex, setDragIndex] = useState<number | null>(null);
	const [overIndex, setOverIndex] = useState<number | null>(null);

	// Installed sources sort alphabetically by name; the pinned section sorts separately by pinOrder
	const all = useMemo(() => [...(sources.data ?? [])].sort(compareSourceName), [sources.data]);
	// Internal sources (local/opds instances) group into their own section; pinning stays an opt-in on top
	const internalSources = all.filter((s) => s.kind !== 'aidoku');
	// Aidoku-style separate section: updatable sources grouped together, below the pinned section and above the full list
	const updateSources = all.filter((s) => s.updateVersion !== null);
	const updateCount = updateSources.length;
	const pinnedSources = all
		.filter((s) => s.pinned)
		.sort((a, b) => (a.pinOrder ?? 0) - (b.pinOrder ?? 0));

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
		const keys = pinnedSources.map((s) => s.key);
		const [moved] = keys.splice(from, 1);
		keys.splice(target, 0, moved);
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
					!searching && tab === 'installed' ? (
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
							<Button onClick={() => setRegisteringInternal(true)}>
								<Plus size={15} />
								{t('browse.addInternal')}
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

			<SearchPanel
				active={searching}
				initialQuery={searchParams.get('q') ?? ''}
				onActiveChange={(active, query) => {
					if (active === searching && query === (searchParams.get('q') ?? '')) return;
					// replace, not push: drilling into a result and back must restore the search, not walk old submissions
					setSearchParams(active ? (query ? { view: 'search', q: query } : { view: 'search' }) : {}, {
						replace: true
					});
				}}
			/>

			{!searching && (
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
			)}

			{!searching && (tab === 'repos' ? (
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
										dnd={{
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
										}}
										onTogglePin={(s) => void togglePin(s)}
										onToggleEnabled={(s) => void toggleEnabled(s)}
										onDelete={setPendingDelete}
									/>
								))}
							</div>
						</>
					)}
					{internalSources.length > 0 && (
						<>
							<h2 className="browse__section">{t('browse.sectionInternal')}</h2>
							<div className="source-list">
								{internalSources.map((s) => (
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
					{(pinnedSources.length > 0 || internalSources.length > 0 || updateSources.length > 0) && (
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
			))}

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
			<InternalSourceModal open={registeringInternal} onClose={() => setRegisteringInternal(false)} />
		</div>
	);
}
