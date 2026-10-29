import { useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink, Plus, RefreshCw, Search } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import {
	TRACKER_PROVIDER_NAMES,
	availableTrackerProviders,
	buildTrackerSave,
	deleteTrackerLink,
	hasTrackUpdate,
	invalidateTrackerLinks,
	isTrackerAuthError,
	listTrackerLinks,
	mergeTrackEditValues,
	oauthStartUrl,
	putTrackerLink,
	remoteToTrackEdit,
	searchTracker,
	syncTrackerLink,
	trackerEntryUrl,
	trackerKeys,
	trackerMediaLabelKey,
	trackerState,
	updateTrackEditValue,
	updateTracker
} from '../../api/trackers';
import { qk } from '../../api/hooks';
import type {
	RemoteState,
	TrackSearchItem,
	TrackStatus,
	TrackerLink,
	TrackerProvider,
	TrackerSyncResult
} from '../../api/types';
import { Badge, Button, Cover, Input, Modal, SegmentedControl, Skeleton, Spinner, pushToast } from '..';
import { TrackerLogo } from './TrackerLogo';
import './trackers.css';

const TRACK_MODES = ['auto', 'chapter', 'volume'] as const satisfies readonly TrackerLink['trackMode'][];
const TRACK_MODE_LABELS: Record<TrackerLink['trackMode'], string> = {
	auto: 'modeAuto',
	chapter: 'modeChapter',
	volume: 'modeVolume'
};
const TRACK_STATUSES = ['reading', 'planning', 'completed', 'paused', 'dropped', 'rereading'] as const satisfies readonly TrackStatus[];

function TrackerAuthNotice({ provider, reconnect = false }: { provider: TrackerProvider; reconnect?: boolean }) {
	const { t } = useTranslation();
	const name = TRACKER_PROVIDER_NAMES[provider];
	return (
		<div className="tracker-auth" role="alert">
			<p>{t('trackers.authRequired', { provider: name })}</p>
			<Button size="sm" variant="primary" onClick={() => window.location.assign(oauthStartUrl(provider))}>
				{reconnect ? t('trackers.reconnect') : t('trackers.connect')}
			</Button>
		</div>
	);
}

function TrackerPill({ link, onClick }: { link: TrackerLink; onClick: () => void }) {
	const { t } = useTranslation();
	const stateQuery = useQuery({
		queryKey: trackerKeys.state(link.provider, link.trackId),
		queryFn: () => trackerState(link.provider, link.trackId),
		retry: false,
		refetchOnWindowFocus: false
	});
	const disconnected = isTrackerAuthError(stateQuery.error);
	const provider = TRACKER_PROVIDER_NAMES[link.provider];
	const status = stateQuery.data?.status;
	return (
		<button
			type="button"
			className={disconnected ? 'tracker-pill tracker-pill--disconnected' : 'tracker-pill'}
			onClick={onClick}
			title={disconnected ? t('trackers.authRequired', { provider }) : undefined}
		>
			<TrackerLogo provider={link.provider} />
			<span className="tracker-pill__provider">{provider}</span>
			{!stateQuery.isPending && !disconnected && status && (
				<span className="tracker-pill__status">{t(`trackers.statusValues.${status}`)}</span>
			)}
			{disconnected && <span className="tracker-pill__auth" aria-label={t('trackers.authRequired', { provider })} />}
		</button>
	);
}

interface AddTrackerDialogProps {
	mangaId: number;
	mangaTitle: string;
	providers: TrackerProvider[];
	onClose: () => void;
	onBound: (link: TrackerLink) => void;
}

function AddTrackerDialog({ mangaId, mangaTitle, providers, onClose, onBound }: AddTrackerDialogProps) {
	const { t } = useTranslation();
	const qc = useQueryClient();
	const [provider, setProvider] = useState<TrackerProvider>(providers[0] ?? 'anilist');
	const [draft, setDraft] = useState(mangaTitle);
	const [submitted, setSubmitted] = useState('');
	const searchQuery = useQuery({
		queryKey: trackerKeys.search(provider, submitted),
		queryFn: () => searchTracker(provider, submitted),
		enabled: submitted.length > 0,
		retry: false
	});
	const bind = useMutation({
		mutationFn: (item: TrackSearchItem) =>
			putTrackerLink(mangaId, { provider, trackId: item.id, title: item.title }),
		onSuccess: (link) => {
			invalidateTrackerLinks(qc, mangaId);
			onBound(link);
		}
	});

	function submitSearch(event?: FormEvent) {
		event?.preventDefault();
		const name = draft.trim();
		if (!name) {
			setSubmitted('');
			return;
		}
		if (name === submitted) void searchQuery.refetch();
		else setSubmitted(name);
	}

	return (
		<Modal open title={t('trackers.addBinding')} onClose={onClose} panelClassName="modal__panel--wide">
			<div className="tracker-add">
				<div className="tracker-add__providers">
					<SegmentedControl
						value={provider}
						disabled={bind.isPending}
						options={providers.map((item) => ({
							value: item,
							label: (
								<span className="tracker-seg__option">
									<TrackerLogo provider={item} />
									<span>{TRACKER_PROVIDER_NAMES[item]}</span>
								</span>
							)
						}))}
						onChange={(item) => {
							setProvider(item);
							setDraft(mangaTitle);
							setSubmitted('');
						}}
					/>
				</div>
				<form className="tracker-search" onSubmit={submitSearch}>
					<Input
						icon={<Search size={15} />}
						type="search"
						value={draft}
						aria-label={t('common.search')}
						onChange={(event) => setDraft(event.currentTarget.value)}
					/>
					<Button type="submit" variant="primary" loading={searchQuery.isFetching && submitted.length > 0}>
						{t('common.search')}
					</Button>
				</form>
				{searchQuery.isPending && submitted ? (
					<div className="tracker-dialog-state">
						<Spinner />
						{t('trackers.searching')}
					</div>
				) : searchQuery.isError ? (
					isTrackerAuthError(searchQuery.error) ? (
						<TrackerAuthNotice provider={provider} />
					) : (
						<div className="tracker-dialog-state tracker-error" role="alert">
							<span>{searchQuery.error.message}</span>
							<Button size="sm" onClick={() => void searchQuery.refetch()}>
								{t('trackers.retry')}
							</Button>
						</div>
					)
				) : searchQuery.data ? (
					searchQuery.data.length === 0 ? (
						<p className="tracker-dialog-state muted">{t('trackers.noResults')}</p>
					) : (
						<ul className="tracker-results">
							{searchQuery.data.map((item) => (
								<li className="tracker-result" key={item.id}>
									<div className="tracker-result__cover">
										<Cover src={item.coverUrl} title={item.title} />
									</div>
									<div className="tracker-result__meta">
										<h4>{item.title}</h4>
										<div className="tracker-result__badges">
											<MediaBadge item={item} />
											{item.tracked && <Badge>{t('trackers.tracked')}</Badge>}
										</div>
									</div>
									<Button size="sm" disabled={bind.isPending} onClick={() => bind.mutate(item)}>
										{t('trackers.bind')}
									</Button>
								</li>
							))}
						</ul>
					)
				) : null}
			</div>
		</Modal>
	);
}

interface TrackerEditFormProps {
	link: TrackerLink;
	initialState: RemoteState | null;
	onClose: () => void;
}

export function TrackerEditForm({ link, initialState, onClose }: TrackerEditFormProps) {
	const { t } = useTranslation();
	const qc = useQueryClient();
	const [currentLink, setCurrentLink] = useState(link);
	const [edits, setEdits] = useState<Partial<ReturnType<typeof remoteToTrackEdit>>>({});
	const values = mergeTrackEditValues(initialState, edits);
	const [trackMode, setTrackMode] = useState<TrackerLink['trackMode']>(link.trackMode);
	const [chapterOffset, setChapterOffset] = useState(String(link.chapterOffset));
	const [confirmUnlink, setConfirmUnlink] = useState(false);
	const provider = TRACKER_PROVIDER_NAMES[link.provider];
	const save = useMutation({
		mutationFn: async () => {
			const plan = buildTrackerSave(currentLink, trackMode, Number(chapterOffset), initialState, values);
			if (plan.link) setCurrentLink(await putTrackerLink(currentLink.mangaId, plan.link));
			if (hasTrackUpdate(plan.remoteUpdate)) {
				await updateTracker(link.provider, { trackId: link.trackId, ...plan.remoteUpdate });
			}
		},
		onSuccess: () => {
			pushToast(t('trackers.saved'), 'info');
			onClose();
		},
		onSettled: () => {
			invalidateTrackerLinks(qc, link.mangaId);
			void qc.invalidateQueries({ queryKey: trackerKeys.state(link.provider, link.trackId) });
		}
	});
	const unlink = useMutation({
		mutationFn: () => deleteTrackerLink(link.mangaId, link.provider),
		onSuccess: () => {
			invalidateTrackerLinks(qc, link.mangaId);
			setConfirmUnlink(false);
			pushToast(t('trackers.unlinked'), 'info');
			onClose();
		}
	});
	// Aidoku's "Sync Local History": preview what a pull from this tracker
	// would mark read, ask, then apply. Applying pushes the new progress to
	// the manga's other bindings server-side (the source tracker is skipped).
	const [syncPreview, setSyncPreview] = useState<TrackerSyncResult | null>(null);
	const applySync = useMutation({
		mutationFn: () => syncTrackerLink(currentLink.mangaId, currentLink.provider),
		onSuccess: (result) => {
			pushToast(t('trackers.syncDone', { count: result.marked }), 'info');
			setSyncPreview(null);
			onClose();
		},
		onSettled: () => {
			void qc.invalidateQueries({ queryKey: qk.chapters(currentLink.mangaId) });
			void qc.invalidateQueries({ queryKey: trackerKeys.state(link.provider, link.trackId) });
			invalidateTrackerLinks(qc, link.mangaId);
		}
	});
	const previewSync = useMutation({
		mutationFn: () => syncTrackerLink(currentLink.mangaId, currentLink.provider, true),
		onSuccess: (plan) => {
			applySync.reset();
			setSyncPreview(plan);
		}
	});
	const saveAuthError = isTrackerAuthError(save.error);

	function updateValue<K extends keyof typeof values>(field: K, value: (typeof values)[K]) {
		setEdits((current) => updateTrackEditValue(initialState, current, field, value));
	}

	function submit(event: FormEvent) {
		event.preventDefault();
		save.mutate();
	}

	return (
		<>
			<form className="tracker-edit" onSubmit={submit}>
				{save.isError &&
					(saveAuthError ? (
						<TrackerAuthNotice provider={link.provider} reconnect />
					) : (
						<p className="tracker-error" role="alert">
							{save.error.message}
						</p>
					))}
				<div className="tracker-edit__mode">
					<SegmentedControl
						options={TRACK_MODES.map((mode) => ({ value: mode, label: t(`trackers.${TRACK_MODE_LABELS[mode]}`) }))}
						value={trackMode}
						onChange={setTrackMode}
					/>
				</div>
				<label className="tracker-field">
					<span>{t('trackers.chapterOffset')}</span>
					<Input
						type="number"
						min={-10000}
						max={10000}
						step={1}
						required
						value={chapterOffset}
						onChange={(event) => setChapterOffset(event.currentTarget.value)}
					/>
					<span className="tracker-field__hint">{t('trackers.chapterOffsetHint')}</span>
				</label>
				<div className="tracker-edit__remote">
					<div className="tracker-edit__remote-head">
						<div className="tracker-edit__remote-id">
							<span>{t('trackers.remoteEntry')}</span>
							<span className="tracker-edit__track-id">#{link.trackId}</span>
						</div>
						<a
							className="tracker-edit__external"
							href={trackerEntryUrl(link.provider, link.trackId)}
							target="_blank"
							rel="noreferrer"
						>
							{t('trackers.openEntry', { provider })}
							<ExternalLink size={14} />
						</a>
					</div>
					{initialState && (
						<div className="tracker-edit__remote-fields">
							<div className="tracker-edit__status">
								<SegmentedControl
									options={TRACK_STATUSES.map((status) => ({
										value: status as TrackStatus | '',
										label: t(`trackers.statusValues.${status}`)
									}))}
									value={values.status}
									onChange={(status) => {
										if (status) updateValue('status', status);
									}}
								/>
							</div>
							<div className="tracker-edit__progress">
								<label className="tracker-field">
									<span>{t('trackers.score')}</span>
									<Input
										type="number"
										min={0}
										step={1}
										value={values.score}
										onChange={(event) => updateValue('score', event.currentTarget.value)}
									/>
								</label>
								<label className="tracker-field">
									<span>{t('trackers.chapterProgress')}</span>
									<Input
										type="number"
										min={0}
										step="any"
										value={values.lastReadChapter}
										onChange={(event) => updateValue('lastReadChapter', event.currentTarget.value)}
									/>
								</label>
								<label className="tracker-field">
									<span>{t('trackers.volumeProgress')}</span>
									<Input
										type="number"
										min={0}
										step={1}
										value={values.lastReadVolume}
										onChange={(event) => updateValue('lastReadVolume', event.currentTarget.value)}
									/>
								</label>
							</div>
						</div>
					)}
				</div>
				{previewSync.isError &&
					(isTrackerAuthError(previewSync.error) ? (
						<TrackerAuthNotice provider={link.provider} reconnect />
					) : (
						<p className="tracker-error" role="alert">
							{previewSync.error.message}
						</p>
					))}
				<div className="tracker-edit__actions">
					<Button variant="danger" onClick={() => setConfirmUnlink(true)}>
						{t('trackers.unlink')}
					</Button>
					<div className="tracker-edit__save">
						<Button onClick={() => previewSync.mutate()} loading={previewSync.isPending}>
							<RefreshCw size={14} />
							{t('trackers.syncFromTracker')}
						</Button>
						<Button onClick={onClose}>{t('common.cancel')}</Button>
						<Button type="submit" variant="primary" loading={save.isPending}>
							{t('common.save')}
						</Button>
					</div>
				</div>
			</form>
			<Modal
				open={syncPreview !== null}
				title={t('trackers.syncTitle')}
				onClose={() => setSyncPreview(null)}
				footer={
					syncPreview !== null && syncPreview.marked > 0 ? (
						<>
							<Button onClick={() => setSyncPreview(null)}>{t('common.cancel')}</Button>
							<Button variant="primary" loading={applySync.isPending} onClick={() => applySync.mutate()}>
								{t('trackers.syncFromTracker')}
							</Button>
						</>
					) : (
						<Button variant="primary" onClick={() => setSyncPreview(null)}>
							{t('common.cancel')}
						</Button>
					)
				}
			>
				{syncPreview !== null &&
					(syncPreview.marked > 0 ? (
						<p>{t('trackers.syncConfirm', { count: syncPreview.marked, provider, progress: syncPreview.progress })}</p>
					) : (
						<p>{t('trackers.syncUpToDate', { provider })}</p>
					))}
				{applySync.isError && (
					<p className="tracker-error" role="alert">
						{applySync.error.message}
					</p>
				)}
			</Modal>
			<Modal
				open={confirmUnlink}
				title={t('trackers.unlinkTitle')}
				onClose={() => setConfirmUnlink(false)}
				footer={
					<>
						<Button onClick={() => setConfirmUnlink(false)}>{t('common.cancel')}</Button>
						<Button variant="danger" loading={unlink.isPending} onClick={() => unlink.mutate()}>
							{t('trackers.unlink')}
						</Button>
					</>
				}
			>
				<p>{t('trackers.unlinkConfirm', { provider, title: link.title ?? link.trackId })}</p>
			</Modal>
		</>
	);
}

function EditTrackerDialog({ link, onClose }: { link: TrackerLink; onClose: () => void }) {
	const { t } = useTranslation();
	const remoteQuery = useQuery({
		queryKey: trackerKeys.state(link.provider, link.trackId),
		queryFn: () => trackerState(link.provider, link.trackId),
		retry: false,
		refetchOnWindowFocus: false
	});
	const provider = TRACKER_PROVIDER_NAMES[link.provider];
	return (
		<Modal
			open
			title={t('trackers.editBinding', { provider })}
			onClose={onClose}
			panelClassName="modal__panel--wide"
		>
			{remoteQuery.isPending ? (
				<div className="tracker-dialog-state">
					<Spinner />
					{t('common.loading')}
				</div>
			) : (
				<>
					{remoteQuery.isError &&
						(isTrackerAuthError(remoteQuery.error) ? (
							<TrackerAuthNotice provider={link.provider} reconnect />
						) : (
							<div className="tracker-dialog-state tracker-error" role="alert">
								<span>{remoteQuery.error.message}</span>
								<Button size="sm" onClick={() => void remoteQuery.refetch()}>
									{t('trackers.retry')}
								</Button>
							</div>
						))}
					<TrackerEditForm link={link} initialState={remoteQuery.data ?? null} onClose={onClose} />
				</>
			)}
		</Modal>
	);
}

export function TrackerSection({ mangaId, mangaTitle }: { mangaId: number; mangaTitle: string }) {
	const { t } = useTranslation();
	const [adding, setAdding] = useState(false);
	const [editing, setEditing] = useState<TrackerLink | null>(null);
	const linksQuery = useQuery({
		queryKey: trackerKeys.mangaLinks(mangaId),
		queryFn: () => listTrackerLinks(mangaId)
	});
	const links = linksQuery.data ?? [];
	const providers = availableTrackerProviders(links);
	const canAdd = linksQuery.isSuccess && providers.length > 0;
	return (
		<section className="tracker-section">
			<div className="tracker-section__head">
				<h2>{t('trackers.sectionTitle')}</h2>
				<Button
					size="sm"
					disabled={!canAdd}
					title={linksQuery.isSuccess && providers.length === 0 ? t('trackers.allProvidersBound') : undefined}
					onClick={() => setAdding(true)}
				>
					<Plus size={15} />
					{t('trackers.addBinding')}
				</Button>
			</div>
			{linksQuery.isPending ? (
				<Skeleton height={48} />
			) : linksQuery.isError ? (
				<p className="tracker-error">{linksQuery.error.message}</p>
			) : links.length === 0 ? (
				<p className="tracker-section__empty">{t('trackers.noMangaBindings')}</p>
			) : (
				<div className="tracker-pills">
					{links.map((link) => (
						<TrackerPill key={link.provider} link={link} onClick={() => setEditing(link)} />
					))}
				</div>
			)}
			{adding && canAdd && (
				<AddTrackerDialog
					mangaId={mangaId}
					mangaTitle={mangaTitle}
					providers={providers}
					onClose={() => setAdding(false)}
					onBound={(link) => {
						setAdding(false);
						setEditing(link);
					}}
				/>
			)}
			{editing && <EditTrackerDialog key={`${editing.provider}:${editing.trackId}`} link={editing} onClose={() => setEditing(null)} />}
		</section>
	);
}

function MediaBadge({ item }: { item: TrackSearchItem }) {
	const { t } = useTranslation();
	const key = trackerMediaLabelKey(item);
	return key ? <Badge>{t(key)}</Badge> : null;
}
