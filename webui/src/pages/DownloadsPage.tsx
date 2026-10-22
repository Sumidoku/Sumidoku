import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Download as DownloadIcon, Pause, Play, RotateCw, Trash2 } from 'lucide-react';
import { api, chapterFileUrl } from '../api/client';
import {
	useDeleteDownload,
	useDownloadsClear,
	useDownloadsStart,
	useDownloadsStop,
	useEnqueueDownloads
} from '../api/hooks';
import type { Download, DownloadStatus } from '../api/types';
import type { DownloadWsFrame } from '../api/protocol';
import { chapterLabel } from '../lib/format';
import {
	Button,
	Chip,
	EmptyState,
	IconButton,
	PageHeader,
	ProgressBar,
	Skeleton,
	pushToast
} from '../components';
import type { ChipColor } from '../components';
import { subscribeWS } from '../api/ws';
import './downloads.css';

const ORDER: DownloadStatus[] = ['queued', 'downloading', 'paused', 'error', 'finished'];

const STATUS_META: Record<DownloadStatus, { labelKey: string; color: ChipColor }> = {
	queued: { labelKey: 'downloads.statusQueued', color: 'default' },
	downloading: { labelKey: 'downloads.statusDownloading', color: 'accent' },
	paused: { labelKey: 'downloads.statusPaused', color: 'warning' },
	finished: { labelKey: 'downloads.statusFinished', color: 'success' },
	error: { labelKey: 'downloads.statusError', color: 'danger' }
};

// a progress frame can also arrive before started; any progress means downloading
const EVENT_STATUS: Record<string, DownloadStatus> = {
	queued: 'queued',
	started: 'downloading',
	progress: 'downloading',
	paused: 'paused',
	finished: 'finished',
	error: 'error'
};

export default function DownloadsPage() {
	const { t } = useTranslation();
	const [items, setItems] = useState<Download[]>([]);
	const [running, setRunning] = useState(false);
	const [online, setOnline] = useState(false);
	const [loading, setLoading] = useState(true);
	const [busy, setBusy] = useState(false);

	const start = useDownloadsStart();
	const stop = useDownloadsStop();
	const clear = useDownloadsClear();
	const del = useDeleteDownload();
	const enqueue = useEnqueueDownloads();

	// WS callbacks outside the effect can't close over the latest list, so sync via a ref; all writes go through applyItems
	const itemsRef = useRef<Download[]>([]);
	function applyItems(next: Download[] | ((cur: Download[]) => Download[])) {
		const value = typeof next === 'function' ? next(itemsRef.current) : next;
		itemsRef.current = value;
		setItems(value);
	}

	// Events carry only chapterId; an unseen task needs a full-list fetch to fill in its title, and concurrent events coalesce into one fetch
	const refetchingRef = useRef(false);
	async function refreshMissing() {
		if (refetchingRef.current) return;
		refetchingRef.current = true;
		try {
			applyItems(await api<Download[]>('/downloads'));
		} finally {
			refetchingRef.current = false;
		}
	}

	useEffect(() => {
		function onMessage(msg: unknown) {
			if (!msg || typeof msg !== 'object' || Array.isArray(msg)) return;
			const m = msg as DownloadWsFrame;
			if (m.type === 'snapshot') {
				applyItems(m.downloads);
				setRunning(m.running);
				setLoading(false);
				return;
			}
			if (m.type === 'running') {
				setRunning(m.running);
				return;
			}
			const status = EVENT_STATUS[m.type];
			if (!status) return;
			if (!itemsRef.current.some((it) => it.chapterId === m.chapterId)) {
				void refreshMissing();
				return;
			}
			applyItems((cur) =>
				cur.map((it) => {
					if (it.chapterId !== m.chapterId) return it;
					const patch: Partial<Download> = { status };
					if (status === 'downloading' || status === 'finished') {
						patch.downloaded = m.downloaded;
						patch.total = m.total;
						if (status === 'finished') {
							patch.error = null;
							if (m.total > 0) patch.downloaded = m.total;
						}
					} else {
						patch.downloaded = null;
						patch.total = null;
						if (status === 'queued') patch.error = null;
					}
					return { ...it, ...patch };
				})
			);
		}

		const unsub = subscribeWS('/api/v1/downloads/ws', { onMessage, onStatus: setOnline });
		void (async () => {
			try {
				applyItems(await api<Download[]>('/downloads'));
			} finally {
				setLoading(false);
			}
		})();
		return unsub;
		// applyItems/refreshMissing only touch refs and setState, so the first-render closure is fine; subscribe once
	}, []);

	const sorted = useMemo(
		() =>
			[...items].sort(
				(a, b) => ORDER.indexOf(a.status) - ORDER.indexOf(b.status) || b.updatedAt - a.updatedAt
			),
		[items]
	);

	// The server's clear deletes error entries along with finished ones; the button is disabled when nothing is clearable
	const hasClearable = items.some((it) => it.status === 'finished' || it.status === 'error');

	async function toggleRunning() {
		setBusy(true);
		try {
			const res = await (running ? stop : start).mutateAsync();
			setRunning(res.running);
		} finally {
			setBusy(false);
		}
	}

	async function clearFinished() {
		setBusy(true);
		try {
			// The server's clear deletes error entries along with finished ones; keep the local list in step
			const res = await clear.mutateAsync();
			applyItems((cur) => cur.filter((it) => it.status !== 'finished' && it.status !== 'error'));
			pushToast(t('downloads.cleared', { count: res.cleared }), 'info');
		} finally {
			setBusy(false);
		}
	}

	return (
		<div className="page container">
			<PageHeader
				title={t('common.nav.downloads')}
				actions={
					<>
						<span
							className={online ? 'ws-dot ws-dot--on' : 'ws-dot'}
							title={online ? t('downloads.wsOk') : t('downloads.wsDown')}
						/>
						<Button
							variant={running ? 'ghost' : 'primary'}
							disabled={busy}
							onClick={() => void toggleRunning()}
						>
							{running ? <Pause size={14} /> : <Play size={14} />}
							{running ? t('downloads.pause') : t('downloads.start')}
						</Button>
						<Button disabled={busy || !hasClearable} onClick={() => void clearFinished()}>
							{t('downloads.clearFinished')}
						</Button>
					</>
				}
			/>

			{loading ? (
				<div className="downloads-list">
					{Array.from({ length: 4 }, (_, i) => (
						<Skeleton key={i} height={72} />
					))}
				</div>
			) : sorted.length === 0 ? (
				<EmptyState
					icon={DownloadIcon}
					title={t('downloads.emptyTitle')}
					hint={t('downloads.emptyHint')}
				/>
			) : (
				<div className="downloads-list">
					{sorted.map((it) => {
						const meta = STATUS_META[it.status];
						const chapterName = chapterLabel({
							volume_number: it.volumeNumber,
							chapter_number: it.chapterNumber,
							title: it.chapterTitle
						});
						const total = it.total ?? 0;
						const downloaded = it.downloaded ?? 0;
						return (
							<div key={it.chapterId} className="downloads-item card">
								<div className="downloads-item__main">
									<div className="downloads-item__row">
										<span className="downloads-item__manga">{it.mangaTitle}</span>
										<Chip color={meta.color}>{t(meta.labelKey)}</Chip>
									</div>
									<div className="muted downloads-item__chapter">{chapterName}</div>
									{it.status === 'downloading' && (
										<div className="downloads-item__progress">
											<ProgressBar value={total > 0 ? downloaded / total : null} />
											<span className="faint downloads-item__pv">
												{total > 0 ? `${downloaded} / ${total}` : `${downloaded}`}
											</span>
										</div>
									)}
									{it.status === 'error' && (
										<div className="downloads-item__error">
											{it.error || t('downloads.downloadFailed')}
											{it.retries > 0 && t('downloads.retried', { count: it.retries })}
										</div>
									)}
								</div>
								<div className="downloads-item__acts">
									{it.status === 'error' && (
										<IconButton
											label={t('downloads.retry')}
											disabled={enqueue.isPending}
											onClick={() =>
												enqueue.mutate([it.chapterId], {
													onSuccess: () =>
														applyItems((cur) =>
															cur.map((x) =>
																x.chapterId === it.chapterId
																	? { ...x, status: 'queued', error: null }
																	: x
															)
														)
												})
											}
										>
											<RotateCw size={16} />
										</IconButton>
									)}
									{it.status === 'finished' && (
										<a
											className="downloads-item__cbz"
											href={chapterFileUrl(it.chapterId)}
											download
											title={t('downloads.downloadCbz')}
										>
											<DownloadIcon size={14} /> CBZ
										</a>
									)}
									{it.status !== 'error' && (
										<IconButton
											label={t('downloads.deleteTask')}
											variant="danger"
											disabled={del.isPending}
											onClick={() =>
												del.mutate(it.chapterId, {
													onSuccess: () => {
														applyItems((cur) => cur.filter((x) => x.chapterId !== it.chapterId));
														pushToast(t('downloads.deleted'), 'info');
													}
												})
											}
										>
											<Trash2 size={16} />
										</IconButton>
									)}
								</div>
							</div>
						);
					})}
				</div>
			)}
		</div>
	);
}
