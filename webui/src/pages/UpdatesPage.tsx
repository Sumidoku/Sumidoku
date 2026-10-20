import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { ApiError, api, coverUrl } from '../api/client';
import { useCategories, useFetchUpdate, useUpdateRecent } from '../api/hooks';
import type { UpdateJob, UpdateJobStatus, UpdateSummary, UpdateWsFrame } from '../api/types';
import { subscribeWS } from '../api/ws';
import { chapterLabel, relativeTime } from '../lib/format';
import {
	Badge,
	Button,
	Cover,
	InfiniteList,
	PageHeader,
	Select,
	Skeleton,
	Spinner,
	pushToast
} from '../components';
import './updates.css';

const JOB_LABEL: Record<UpdateJobStatus, string> = {
	pending: '等待',
	running: '更新中',
	complete: '完成',
	failed: '失败',
	skipped: '跳过'
};

interface FeedJob extends UpdateJob {
	seq: number;
}

export default function UpdatesPage() {
	const [summary, setSummary] = useState<UpdateSummary | null>(null);
	const [summaryLoaded, setSummaryLoaded] = useState(false);
	const [online, setOnline] = useState(false);
	const [updating, setUpdating] = useState(false);
	const [catSel, setCatSel] = useState('');
	const [jobs, setJobs] = useState<FeedJob[]>([]);
	const jobSeq = useRef(0);

	const categories = useCategories();
	const fetchUpdate = useFetchUpdate();
	const recent = useUpdateRecent();
	const recentEntries = useMemo(
		() => recent.data?.pages.flatMap((p) => p.entries) ?? [],
		[recent.data]
	);

	useEffect(() => {
		function onMessage(msg: unknown) {
			if (!msg || typeof msg !== 'object' || Array.isArray(msg)) return;
			const m = msg as UpdateWsFrame;
			if (m.type === 'summary') {
				const { type: _type, ...s } = m;
				setSummary(s);
				setSummaryLoaded(true);
				return;
			}
			if (m.type !== 'job') return;
			const { type: _t, ...job } = m;
			// Consecutive events for the same manga keep only the newest, so the live list doesn't get flooded
			setJobs((cur) =>
				[{ ...job, seq: ++jobSeq.current }, ...cur.filter((j) => j.mangaId !== job.mangaId)].slice(
					0,
					50
				)
			);
			// A job event means work is running; the contract has no round-finished event, so running settles via polling reconciliation
			setSummary((s) => (s && !s.running ? { ...s, running: true } : s));
		}

		const unsub = subscribeWS('/api/v1/update/ws', { onMessage, onStatus: setOnline });
		void (async () => {
			try {
				setSummary(await api<UpdateSummary>('/update/summary'));
			} finally {
				setSummaryLoaded(true);
			}
		})();
		return unsub;
	}, []);

	// While running, poll the summary every 4s to reconcile: refresh counts + settle running (the WS pushes no round-finished event)
	useEffect(() => {
		if (!summary?.running) return;
		const t = setInterval(async () => {
			try {
				setSummary(await api<UpdateSummary>('/update/summary', { silent: true }));
			} catch {
				// On failure, wait for the next round
			}
		}, 4000);
		return () => clearInterval(t);
	}, [summary?.running]);

	async function runUpdate(categoryId?: number) {
		if (updating) return;
		setUpdating(true);
		try {
			await fetchUpdate.mutateAsync(categoryId);
			pushToast('已开始更新', 'info');
			setSummary((s) => (s ? { ...s, running: true } : s));
		} catch (e) {
			if (e instanceof ApiError && e.status === 409) pushToast('已有更新任务在进行', 'info');
			else pushToast(e instanceof Error ? e.message : '更新失败');
		} finally {
			setUpdating(false);
		}
	}

	const running = summary?.running === true;
	const cats = categories.data ?? [];

	return (
		<div className="page container">
			<PageHeader title="更新" />

			{!summaryLoaded ? (
				<Skeleton height={150} />
			) : !summary ? (
				<div className="card muted">更新摘要不可用</div>
			) : (
				<div className="card updates-summary">
					<div className="updates-summary__head">
						{summary.running ? (
							<>
								<Spinner />
								<span className="updates-summary__status">正在更新</span>
							</>
						) : (
							<span className="updates-summary__status updates-summary__status--idle">
								{summary.lastRunAt ? `上次更新 ${relativeTime(summary.lastRunAt)}` : '空闲'}
							</span>
						)}
					</div>
					<div className="updates-summary__counts">
						<div className="updates-stat">
							<div className="updates-stat__v">{summary.total}</div>
							<div className="updates-stat__k">总数</div>
						</div>
						<div className="updates-stat updates-stat--ok">
							<div className="updates-stat__v">{summary.complete}</div>
							<div className="updates-stat__k">完成</div>
						</div>
						<div className="updates-stat updates-stat--bad">
							<div className="updates-stat__v">{summary.failed}</div>
							<div className="updates-stat__k">失败</div>
						</div>
						<div className="updates-stat">
							<div className="updates-stat__v">{summary.skipped}</div>
							<div className="updates-stat__k">跳过</div>
						</div>
						<div className="updates-stat">
							<div className="updates-stat__v">{summary.pending}</div>
							<div className="updates-stat__k">待处理</div>
						</div>
					</div>
					<div className="updates-summary__ops">
						<span
							className={online ? 'ws-dot ws-dot--on' : 'ws-dot'}
							title={online ? '实时连接正常' : '连接已断开，3 秒后重试'}
						/>
						<Button
							variant="primary"
							loading={updating}
							disabled={running}
							onClick={() => void runUpdate()}
						>
							立即更新
						</Button>
						{cats.length > 0 && (
							<>
								<Select
									aria-label="选择分类"
									value={catSel}
									onChange={(e) => setCatSel(e.currentTarget.value)}
								>
									<option value="">全部</option>
									{cats.map((c) => (
										<option key={c.id} value={String(c.id)}>
											{c.name}
										</option>
									))}
								</Select>
								<Button
									disabled={updating || running || catSel === ''}
									onClick={() => void runUpdate(Number(catSel))}
								>
									按分类更新
								</Button>
							</>
						)}
					</div>
				</div>
			)}

			{jobs.length > 0 && (
				<div className="card updates-jobs">
					<div className="faint updates-jobs__title">实时动态</div>
					{jobs.map((j) => (
						<div key={j.seq} className="updates-job">
							<span className={`updates-job__dot updates-job__dot--${j.status}`} />
							<span className="updates-job__title">{j.mangaTitle}</span>
							<span className={`updates-job__status updates-job__status--${j.status}`}>
								{JOB_LABEL[j.status] ?? j.status}
							</span>
							{j.addedChapters > 0 && (
								<span className="updates-job__added">+{j.addedChapters} 章</span>
							)}
						</div>
					))}
				</div>
			)}

			<h2 className="updates-recent__heading">最近更新</h2>
			{recent.isPending ? (
				<div className="updates-list">
					{Array.from({ length: 4 }, (_, i) => (
						<Skeleton key={i} height={76} />
					))}
				</div>
			) : recentEntries.length === 0 ? (
				<div className="updates-empty faint">最近没有新章节</div>
			) : (
				<InfiniteList
					hasNextPage={recent.hasNextPage}
					isFetching={recent.isFetching}
					onLoadMore={() => void recent.fetchNextPage()}
				>
					<div className="updates-list">
						{recentEntries.map((e) => (
							<Link key={e.chapter.id} className="updates-item card" to={`/manga/${e.manga.id}`}>
								<div className="updates-item__thumb">
									<Cover src={coverUrl(e.manga.id, e.manga.cover)} title={e.manga.title} />
								</div>
								<div className="updates-item__meta">
									<div className="updates-item__title">{e.manga.title}</div>
									<div className="muted updates-item__chapter">
										{chapterLabel(e.chapter)}
										<Badge>新章</Badge>
									</div>
									<div className="faint updates-item__time">
										{relativeTime(e.fetchedAt ?? e.fetched_at)}
									</div>
								</div>
							</Link>
						))}
					</div>
				</InfiniteList>
			)}
		</div>
	);
}
