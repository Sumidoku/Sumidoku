import { useEffect, useRef, useState } from 'react';
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router-dom';
import { useManga } from '../../api/hooks';
import {
	TRACKER_PROVIDER_NAMES,
	TRACKER_PROVIDERS,
	listAllTrackerLinks,
	deleteTrackerLink,
	invalidateTrackerLinks,
	logoutTracker,
	oauthStartUrl,
	oauthStatus,
	readOAuthReturn,
	trackerEntryUrl,
	trackerKeys
} from '../../api/trackers';
import type { TrackerLink, TrackerProvider } from '../../api/types';
import { Button, InfiniteList, Modal, Skeleton, pushToast } from '../../components';
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
			pushToast(t('trackers.disconnected', { provider: TRACKER_PROVIDER_NAMES[provider] }), 'info');
		}
	});
	const name = TRACKER_PROVIDER_NAMES[provider];
	const connected = statusQuery.data?.logged_in === true;
	return (
		<article className="tracker-provider">
			<div className="tracker-provider__identity">
				<TrackerLogo provider={provider} />
				<div className="tracker-provider__meta">
					<h3>{name}</h3>
					{statusQuery.isPending ? (
						<Skeleton height={14} width={120} />
					) : statusQuery.isError ? (
						<p className="tracker-provider__state tracker-provider__state--error">
							{statusQuery.error.message}
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

function TrackerBindingRow({ link, onUnbind }: { link: TrackerLink; onUnbind: (link: TrackerLink) => void }) {
	const { t } = useTranslation();
	const mangaQuery = useManga(link.mangaId, { silent: true });
	const mangaTitle = mangaQuery.data?.title;
	const provider = TRACKER_PROVIDER_NAMES[link.provider];
	return (
		<li className="tracker-binding">
			<TrackerLogo provider={link.provider} />
			<div className="tracker-binding__meta">
				{mangaTitle ? (
					<Link className="tracker-binding__manga" to={`/manga/${link.mangaId}`}>
						{mangaTitle}
					</Link>
				) : (
					<span className="tracker-binding__manga">{t('trackers.unknownManga', { id: link.mangaId })}</span>
				)}
				<span className="tracker-binding__remote">
					{provider} · {link.title ?? link.trackId}
				</span>
			</div>
			<a
				className="tracker-binding__external"
				href={trackerEntryUrl(link.provider, link.trackId)}
				target="_blank"
				rel="noreferrer"
				aria-label={t('trackers.openEntry', { provider })}
			>
				<ExternalLink size={16} />
			</a>
			<Button size="sm" onClick={() => onUnbind(link)}>
				{t('trackers.unbind')}
			</Button>
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
	const linksQuery = useInfiniteQuery({
		queryKey: trackerKeys.links,
		queryFn: ({ pageParam }) => listAllTrackerLinks(pageParam),
		initialPageParam: 1,
		getNextPageParam: (last, all) => (last.hasNextPage ? all.length + 1 : undefined)
	});
	const links = linksQuery.data?.pages.flatMap((page) => page.entries) ?? [];
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
					) : links.length === 0 ? (
						<p className="muted">{t('trackers.noBindings')}</p>
					) : (
						<InfiniteList
							hasNextPage={linksQuery.hasNextPage}
							isFetching={linksQuery.isFetching}
							onLoadMore={() => void linksQuery.fetchNextPage()}
						>
							<ul className="tracker-bindings">
								{links.map((link) => (
									<TrackerBindingRow
										key={`${link.mangaId}:${link.provider}`}
										link={link}
										onUnbind={setPendingUnbind}
									/>
								))}
							</ul>
						</InfiniteList>
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
