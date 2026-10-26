import { useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Plus, Search } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import {
	TRACKER_PROVIDER_NAMES,
	availableTrackerProviders,
	buildTrackerSave,
	deleteTrackerLink,
	hasTrackUpdate,
	isTrackerAuthError,
	listTrackerLinks,
	mergeTrackEditValues,
	oauthStartUrl,
	oauthStatus,
	putTrackerLink,
	remoteToTrackEdit,
	searchTracker,
	trackerEntryUrl,
	trackerKeys,
	trackerState,
	updateTracker
} from '../../api/trackers';
import type {
	RemoteState,
	TrackSearchItem,
	TrackStatus,
	TrackerLink,
	TrackerProvider
} from '../../api/types';
import { Badge, Button, Cover, Input, Modal, Select, Skeleton, Spinner, pushToast } from '..';
import { TrackerLogo } from './TrackerLogo';
import './trackers.css';

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
	const accountQuery = useQuery({
		queryKey: trackerKeys.oauthStatus(link.provider),
		queryFn: () => oauthStatus(link.provider)
	});
	const disconnected = accountQuery.data?.logged_in === false;
	const provider = TRACKER_PROVIDER_NAMES[link.provider];
	return (
		<button
			type="button"
			className={disconnected ? 'tracker-pill tracker-pill--disconnected' : 'tracker-pill'}
			onClick={onClick}
			title={disconnected ? t('trackers.authRequired', { provider }) : link.title}
		>
			<TrackerLogo provider={link.provider} />
			<span className="tracker-pill__label">
				<span className="tracker-pill__provider">{provider}</span>
				<span className="tracker-pill__title">{link.title ?? link.trackId}</span>
			</span>
			{disconnected && <span className="tracker-pill__auth">{t('trackers.reconnect')}</span>}
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
			void qc.invalidateQueries({ queryKey: trackerKeys.links });
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
				<label className="tracker-field">
					<span>{t('trackers.provider')}</span>
					<Select
						value={provider}
						disabled={bind.isPending}
						onChange={(event) => {
							setProvider(event.currentTarget.value as TrackerProvider);
							setDraft(mangaTitle);
							setSubmitted('');
						}}
					>
						{providers.map((item) => (
							<option key={item} value={item}>
								{TRACKER_PROVIDER_NAMES[item]}
							</option>
						))}
					</Select>
				</label>
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
											{item.mediaType && <Badge>{t(`trackers.media.${item.mediaType}`)}</Badge>}
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

function TrackerEditForm({ link, initialState, onClose }: TrackerEditFormProps) {
	const { t } = useTranslation();
	const qc = useQueryClient();
	const [edits, setEdits] = useState<Partial<ReturnType<typeof remoteToTrackEdit>>>({});
	const values = mergeTrackEditValues(initialState, edits);
	const [trackMode, setTrackMode] = useState<TrackerLink['trackMode']>(link.trackMode);
	const [chapterOffset, setChapterOffset] = useState(String(link.chapterOffset));
	const [confirmUnlink, setConfirmUnlink] = useState(false);
	const provider = TRACKER_PROVIDER_NAMES[link.provider];
	const save = useMutation({
		mutationFn: async () => {
			const plan = buildTrackerSave(link, trackMode, Number(chapterOffset), initialState, values);
			if (plan.link) await putTrackerLink(link.mangaId, plan.link);
			if (hasTrackUpdate(plan.remoteUpdate)) {
				await updateTracker(link.provider, { trackId: link.trackId, ...plan.remoteUpdate });
			}
		},
		onSuccess: () => {
			pushToast(t('trackers.saved'), 'info');
			onClose();
		},
		onSettled: () => {
			void qc.invalidateQueries({ queryKey: trackerKeys.links });
			void qc.invalidateQueries({ queryKey: trackerKeys.state(link.provider, link.trackId) });
		}
	});
	const unlink = useMutation({
		mutationFn: () => deleteTrackerLink(link.mangaId, link.provider),
		onSuccess: () => {
			void qc.invalidateQueries({ queryKey: trackerKeys.links });
			setConfirmUnlink(false);
			pushToast(t('trackers.unlinked'), 'info');
			onClose();
		}
	});
	const saveAuthError = isTrackerAuthError(save.error);

	function updateValue<K extends keyof typeof values>(field: K, value: (typeof values)[K]) {
		setEdits((current) => ({ ...current, [field]: value }));
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
				<div className="tracker-edit__grid">
					<label className="tracker-field">
						<span>{t('trackers.trackMode')}</span>
						<Select value={trackMode} onChange={(event) => setTrackMode(event.currentTarget.value as TrackerLink['trackMode'])}>
							<option value="auto">{t('trackers.modeAuto')}</option>
							<option value="chapter">{t('trackers.modeChapter')}</option>
							<option value="volume">{t('trackers.modeVolume')}</option>
						</Select>
					</label>
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
					</label>
				</div>
				{initialState && (
					<div className="tracker-edit__remote">
						<div className="tracker-edit__grid">
							<label className="tracker-field">
								<span>{t('trackers.statusLabel')}</span>
								<Select
									value={values.status}
									onChange={(event) => updateValue('status', event.currentTarget.value as TrackStatus | '')}
								>
									{values.status === '' && <option value="">{t('trackers.statusNone')}</option>}
									{TRACK_STATUSES.map((status) => (
										<option key={status} value={status}>
											{t(`trackers.statusValues.${status}`)}
										</option>
									))}
								</Select>
							</label>
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
				<div className="tracker-edit__actions">
					<Button variant="danger" onClick={() => setConfirmUnlink(true)}>
						{t('trackers.unlink')}
					</Button>
					<div className="tracker-edit__save">
						<Button onClick={onClose}>{t('common.cancel')}</Button>
						<Button type="submit" variant="primary" loading={save.isPending}>
							{t('common.save')}
						</Button>
					</div>
				</div>
			</form>
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
			<a
				className="tracker-edit__external"
				href={trackerEntryUrl(link.provider, link.trackId)}
				target="_blank"
				rel="noreferrer"
			>
				{link.title ?? link.trackId}
			</a>
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
