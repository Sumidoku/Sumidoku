import { useEffect, useMemo, useRef, useState } from 'react';
import { Download, Pause, Play, RotateCw, Trash2 } from 'lucide-react';
import { api, chapterFileUrl } from '../api/client';
import {
	useDeleteDownload,
	useDownloadsClear,
	useDownloadsStart,
	useDownloadsStop,
	useEnqueueDownloads
} from '../api/hooks';
import type { DownloadItem, DownloadStatus, DownloadWsFrame } from '../api/types';
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

const STATUS_META: Record<DownloadStatus, { label: string; color: ChipColor }> = {
	queued: { label: '排队中', color: 'default' },
	downloading: { label: '下载中', color: 'accent' },
	paused: { label: '已暂停', color: 'warning' },
	finished: { label: '已完成', color: 'success' },
	error: { label: '失败', color: 'danger' }
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
	const [items, setItems] = useState<DownloadItem[]>([]);
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
	const itemsRef = useRef<DownloadItem[]>([]);
	function applyItems(next: DownloadItem[] | ((cur: DownloadItem[]) => DownloadItem[])) {
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
			applyItems(await api<DownloadItem[]>('/downloads'));
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
					const patch: Partial<DownloadItem> = { status };
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
				applyItems(await api<DownloadItem[]>('/downloads'));
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
			pushToast(`已清空 ${res.cleared} 项`, 'info');
		} finally {
			setBusy(false);
		}
	}

	return (
		<div className="page container">
			<PageHeader
				title="下载"
				actions={
					<>
						<span
							className={online ? 'ws-dot ws-dot--on' : 'ws-dot'}
							title={online ? '实时连接正常' : '连接已断开，3 秒后重试'}
						/>
						<Button
							variant={running ? 'ghost' : 'primary'}
							disabled={busy}
							onClick={() => void toggleRunning()}
						>
							{running ? <Pause size={14} /> : <Play size={14} />}
							{running ? '暂停' : '开始'}
						</Button>
						<Button disabled={busy || !hasClearable} onClick={() => void clearFinished()}>
							清空已完成
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
					icon={Download}
					title="没有下载任务"
					hint="在漫画详情页下载章节后，任务会出现在这里"
				/>
			) : (
				<div className="downloads-list">
					{sorted.map((it) => {
						const meta = STATUS_META[it.status];
						const chapterName = it.chapterTitle?.trim() || '未命名章节';
						const total = it.total ?? 0;
						const downloaded = it.downloaded ?? 0;
						return (
							<div key={it.chapterId} className="downloads-item card">
								<div className="downloads-item__main">
									<div className="downloads-item__row">
										<span className="downloads-item__manga">{it.mangaTitle}</span>
										<Chip color={meta.color}>{meta.label}</Chip>
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
											{it.error || '下载失败'}
											{it.retries > 0 && `（已重试 ${it.retries} 次）`}
										</div>
									)}
								</div>
								<div className="downloads-item__acts">
									{it.status === 'error' && (
										<IconButton
											label="重试"
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
											title="下载 CBZ 文件"
										>
											<Download size={14} /> CBZ
										</a>
									)}
									{it.status !== 'error' && (
										<IconButton
											label="删除任务"
											variant="danger"
											disabled={del.isPending}
											onClick={() =>
												del.mutate(it.chapterId, {
													onSuccess: () => {
														applyItems((cur) => cur.filter((x) => x.chapterId !== it.chapterId));
														pushToast('已删除', 'info');
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
