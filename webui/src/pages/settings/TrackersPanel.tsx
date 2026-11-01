import { useEffect, useRef, useState } from 'react';
import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Trash2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router-dom';
import { coverUrl } from '../../api/client';
import {
	TRACKER_PROVIDER_NAMES,
	TRACKER_PROVIDERS,
	groupBindings,
	listAllTrackerLinks,
	deleteTrackerLink,
	invalidateTrackerLinks,
	logoutTracker,
	oauthStartUrl,
	oauthStatus,
	putTrackerPreferences,
	readOAuthReturn,
	trackerEntryUrl,
	trackerErrorMessage,
	trackerKeys,
	trackerPreferences
} from '../../api/trackers';
import type { TrackerBinding, TrackerLink, TrackerProvider } from '../../api/types';
import { Badge, Button, Cover, IconButton, Modal, SegmentedControl, Skeleton, pushToast } from '../../components';
import { TrackerLogo } from '../../components/trackers/TrackerLogo';
import '../../components/trackers/trackers.css';

function TrackerAccountCard({ provider }: { provider: TrackerProvider }) {
	const { t } = useTranslation();
	const qc = useQueryClient();
	const [confirming, setConfirming] = useState(false);
	const statusQuery = useQuery({
		queryKey: trackerKeys.oauthStatus(provider),
		queryFn: () => oauthStatus(provider)
	});
	const disconnect = useMutation({
		mutationFn: () => logoutTracker(provider),
		onSuccess: () => {
			setConfirming(false);
			void qc.invalidateQueries({ queryKey: trackerKeys.oauth });
			void qc.invalidateQueries({ queryKey: trackerKeys.states });
			void qc.invalidateQueries({ queryKey: trackerKeys.preferences });
			pushToast(t('trackers.disconnected', { provider: TRACKER_PROVIDER_NAMES[provider] }), 'info');
		}
	});
	const preferencesQuery = useQuery({ queryKey: trackerKeys.preferences, queryFn: trackerPreferences });
	const setDefault = useMutation({
		mutationFn: (defaultTracker: TrackerProvider | null) => putTrackerPreferences({ defaultTracker }),
		onSuccess: (preferences) => qc.setQueryData(trackerKeys.preferences, preferences)
	});
	const name = TRACKER_PROVIDER_NAMES[provider];
	const connected = statusQuery.data?.logged_in === true;
	const isDefault = connected && preferencesQuery.data?.defaultTracker === provider;
	return (
		<article className="tracker-provider">
			<div className="tracker-provider__identity">
				<TrackerLogo provider={provider} />
				<div className="tracker-provider__meta">
					<h3>
						{name}
						{isDefault && <Badge>{t('trackers.defaultBadge')}</Badge>}
					</h3>
					{statusQuery.isPending ? (
						<Skeleton height={14} width={120} />
					) : statusQuery.isError ? (
						<p className="tracker-provider__state tracker-provider__state--error">
							{trackerErrorMessage(statusQuery.error)}
						</p>
					) : connected ? (
						<p className="tracker-provider__state">
							{statusQuery.data.username
								? t('trackers.connectedAs', { username: statusQuery.data.username })
								: t('trackers.connected')}
						</p>
					) : (
						<p className="tracker-provider__state">{t('trackers.disconnectedState')}</p>
					)}
				</div>
			</div>
			<div className="tracker-provider__actions">
				{connected && (
					<Button
						size="sm"
						variant="ghost"
						disabled={!preferencesQuery.isSuccess}
						loading={setDefault.isPending}
						aria-pressed={isDefault}
						onClick={() => setDefault.mutate(isDefault ? null : provider)}
					>
						{isDefault ? t('trackers.clearDefault') : t('trackers.setDefault')}
					</Button>
				)}
				<Button
					size="sm"
					variant={connected ? 'ghost' : 'primary'}
					onClick={() => window.location.assign(oauthStartUrl(provider))}
				>
					{connected ? t('trackers.reconnect') : t('trackers.connect')}
				</Button>
				{connected && (
					<Button size="sm" variant="danger" onClick={() => setConfirming(true)}>
						{t('trackers.disconnect')}
					</Button>
				)}
			</div>
			<Modal
				open={confirming}
				title={t('trackers.disconnectTitle', { provider: name })}
				onClose={() => setConfirming(false)}
				footer={
					<>
						<Button onClick={() => setConfirming(false)}>{t('common.cancel')}</Button>
						<Button variant="danger" loading={disconnect.isPending} onClick={() => disconnect.mutate()}>
							{t('trackers.disconnect')}
						</Button>
					</>
				}
			>
				<p>{t('trackers.disconnectConfirm', { provider: name })}</p>
			</Modal>
		</article>
	);
}

function TrackerBindingRow({ group, onUnbind }: { group: TrackerBinding[]; onUnbind: (link: TrackerLink) => void }) {
	const { t } = useTranslation();
	const { mangaId, mangaTitle, mangaCover } = group[0];
	const title = mangaTitle || t('trackers.unknownManga', { id: mangaId });
	return (
		<li className="tracker-binding">
			<Link className="tracker-binding__cover" to={`/manga/${mangaId}`} aria-label={title}>
				<Cover src={coverUrl(mangaId, mangaCover)} title={title} />
			</Link>
			<div className="tracker-binding__meta">
				<Link className="tracker-binding__manga" to={`/manga/${mangaId}`}>
					{title}
				</Link>
				<ul className="tracker-binding__links">
					{group.map((link) => {
						const provider = TRACKER_PROVIDER_NAMES[link.provider];
						return (
							<li className="tracker-binding__link" key={link.provider}>
								<TrackerLogo provider={link.provider} />
								<a
									className="tracker-binding__remote"
									href={trackerEntryUrl(link.provider, link.trackId)}
									target="_blank"
									rel="noreferrer"
									title={t('trackers.openEntry', { provider })}
								>
									{link.title ?? link.trackId}
								</a>
								<IconButton size="sm" label={`${t('trackers.unbind')} ${provider}`} onClick={() => onUnbind(link)}>
									<Trash2 size={14} />
								</IconButton>
							</li>
						);
					})}
				</ul>
			</div>
		</li>
	);
}

export function TrackersPanel() {
	const { t } = useTranslation();
	const qc = useQueryClient();
	const [searchParams, setSearchParams] = useSearchParams();
	const [pendingUnbind, setPendingUnbind] = useState<TrackerLink | null>(null);
	const handledOAuth = useRef<string | null>(null);
	const search = searchParams.toString();
	const [filter, setFilter] = useState<TrackerProvider | 'all'>('all');
	const [page, setPage] = useState(1);
	const provider = filter === 'all' ? undefined : filter;
	const linksQuery = useQuery({
		queryKey: trackerKeys.bindingsPage(page, provider),
		queryFn: () => listAllTrackerLinks(page, provider),
		placeholderData: keepPreviousData
	});
	const data = linksQuery.data;
	const groups = groupBindings(data?.entries ?? []);
	const totalPages = data?.totalPages ?? 0;
	const counts = data?.counts;
	// Unbinding the last manga of the last page leaves the current page past the end
	if (!linksQuery.isPlaceholderData && page > 1 && page > totalPages) setPage(Math.max(1, totalPages));
	const unbind = useMutation({
		mutationFn: (link: TrackerLink) => deleteTrackerLink(link.mangaId, link.provider),
		onSuccess: (_, link) => {
			setPendingUnbind(null);
			invalidateTrackerLinks(qc, link.mangaId);
			pushToast(t('trackers.unbound'), 'info');
		}
	});

	useEffect(() => {
		const result = readOAuthReturn(search);
		if (!result) {
			handledOAuth.current = null;
			return;
		}
		if (handledOAuth.current === search) return;
		handledOAuth.current = search;
		setSearchParams(new URLSearchParams(result.cleanedSearch), { replace: true });
		void qc.invalidateQueries({ queryKey: trackerKeys.oauth });
		if (result.status === 'success') {
			pushToast(t('trackers.oauthSuccess'), 'info');
		} else {
			pushToast(result.message || t('trackers.oauthError'));
		}
	}, [qc, search, setSearchParams, t]);

	return (
		<>
			<section className="settings-section card">
				<h2 className="settings-section__title">{t('trackers.settingsTitle')}</h2>
				<p className="settings-section__desc">{t('trackers.settingsDesc')}</p>
				<div className="settings-section__body tracker-providers">
					{TRACKER_PROVIDERS.map((provider) => (
						<TrackerAccountCard key={provider} provider={provider} />
					))}
				</div>
			</section>
			<section className="settings-section card">
				<h2 className="settings-section__title">{t('trackers.bindingsTitle')}</h2>
				<p className="settings-section__desc">{t('trackers.bindingsDesc')}</p>
				<div className="settings-section__body">
					{linksQuery.isPending ? (
						<div className="tracker-bindings">
							<Skeleton height={56} />
							<Skeleton height={56} />
						</div>
					) : linksQuery.isError ? (
						<p className="tracker-error">{linksQuery.error.message}</p>
					) : !counts?.manga ? (
						<p className="muted">{t('trackers.noBindings')}</p>
					) : (
						<>
							<div className="tracker-bindings__filter">
								<SegmentedControl<TrackerProvider | 'all'>
									value={filter}
									options={[
										{
											value: 'all',
											label: (
												<span className="tracker-seg__option">
													<span>{t('trackers.filterAll')}</span>
													<span className="tracker-seg__count">{counts.manga}</span>
												</span>
											)
										},
										...TRACKER_PROVIDERS.filter((item) => counts.providers[item] || item === filter).map((item) => ({
											value: item,
											label: (
												<span className="tracker-seg__option">
													<TrackerLogo provider={item} />
													<span>{TRACKER_PROVIDER_NAMES[item]}</span>
													<span className="tracker-seg__count">{counts.providers[item] ?? 0}</span>
												</span>
											)
										}))
									]}
									onChange={(next) => {
										setFilter(next);
										setPage(1);
									}}
								/>
							</div>
							<ul className="tracker-bindings">
								{groups.map((group) => (
									<TrackerBindingRow key={group[0].mangaId} group={group} onUnbind={setPendingUnbind} />
								))}
							</ul>
							{totalPages > 1 && (
								<div className="tracker-bindings__pager">
									<span className="muted">{t('trackers.pageInfo', { page, pages: totalPages })}</span>
									<div className="tracker-bindings__pager-actions">
										<Button size="sm" disabled={page <= 1} onClick={() => setPage(page - 1)}>
											{t('trackers.previousPage')}
										</Button>
										<Button
											size="sm"
											disabled={!data?.hasNextPage || linksQuery.isPlaceholderData}
											onClick={() => setPage(page + 1)}
										>
											{t('trackers.nextPage')}
										</Button>
									</div>
								</div>
							)}
						</>
					)}
				</div>
			</section>
			<Modal
				open={pendingUnbind !== null}
				title={t('trackers.unbindTitle')}
				onClose={() => setPendingUnbind(null)}
				footer={
					<>
						<Button onClick={() => setPendingUnbind(null)}>{t('common.cancel')}</Button>
						<Button
							variant="danger"
							loading={unbind.isPending}
							onClick={() => pendingUnbind && unbind.mutate(pendingUnbind)}
						>
							{t('trackers.unbind')}
						</Button>
					</>
				}
			>
				<p>
					{pendingUnbind &&
						t('trackers.unbindConfirm', {
							provider: TRACKER_PROVIDER_NAMES[pendingUnbind.provider],
							title: pendingUnbind.title ?? pendingUnbind.trackId
						})}
				</p>
			</Modal>
		</>
	);
}
