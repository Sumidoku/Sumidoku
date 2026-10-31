import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { ApiError, api, coverUrl } from '../api/client';
import { useCategories, useFetchUpdate, useUpdateRecent } from '../api/hooks';
import type { UpdateSummary } from '../api/types';
import type { UpdateEvent, UpdateJob, UpdateJobStatus } from '../api/protocol';
import { sse } from '../api/sse';
import { translateApiError } from '../i18n';
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

const JOB_LABEL_KEY: Record<UpdateJobStatus, string> = {
	pending: 'updates.jobPending',
	running: 'updates.jobRunning',
	complete: 'updates.complete',
	failed: 'updates.failed',
	skipped: 'updates.skipped'
};

interface FeedJob extends UpdateJob {
	seq: number;
}

export default function UpdatesPage() {
	const { t } = useTranslation();
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
		let receivedEvent = false;

		function onSnapshot(msg: unknown) {
			const m = msg as { updateSummary: UpdateSummary };
			receivedEvent = true;
			setSummary(m.updateSummary);
			setSummaryLoaded(true);
		}

		function onSummary(msg: unknown) {
			receivedEvent = true;
			setSummary(msg as UpdateSummary);
			setSummaryLoaded(true);
		}

		function onComplete(msg: unknown) {
			receivedEvent = true;
			setSummary({ ...(msg as UpdateSummary), running: false });
			setSummaryLoaded(true);
		}

		function onJob(msg: unknown) {
			receivedEvent = true;
			const { type: _t, ...job } = msg as UpdateEvent;
			// Consecutive events for the same manga keep only the newest, so the live list doesn't get flooded
			setJobs((cur) =>
				[{ ...job, seq: ++jobSeq.current }, ...cur.filter((j) => j.mangaId !== job.mangaId)].slice(
					0,
					50
				)
			);
		}

		const unsubStatus = sse.onStatus(setOnline);
		const unsubSnapshot = sse.on('snapshot', onSnapshot);
		const unsubSummary = sse.on('updateSummary', onSummary);
		const unsubComplete = sse.on('updateComplete', onComplete);
		const unsubJob = sse.on('update', onJob);
		void (async () => {
			try {
				const initial = await api<UpdateSummary>('/update/summary');
				if (!receivedEvent) setSummary(initial);
			} finally {
				setSummaryLoaded(true);
			}
		})();
		return () => {
			unsubStatus();
			unsubSnapshot();
			unsubSummary();
			unsubComplete();
			unsubJob();
		};
	}, []);

	async function runUpdate(categoryId?: number) {
		if (updating) return;
		setUpdating(true);
		try {
			await fetchUpdate.mutateAsync(categoryId);
			pushToast(t('updates.started'), 'info');
		} catch (e) {
			if (e instanceof ApiError && e.status === 409) pushToast(t('updates.alreadyRunning'), 'info');
			else if (e instanceof ApiError) pushToast(translateApiError(e.code, e.message));
			else pushToast(e instanceof Error ? e.message : t('updates.updateFailed'));
		} finally {
			setUpdating(false);
		}
	}

	const running = summary?.running === true;
	const cats = categories.data ?? [];

	return (
		<div className="page container">
			<PageHeader title={t('common.nav.updates')} />

			{!summaryLoaded ? (
				<Skeleton height={150} />
			) : !summary ? (
				<div className="card muted">{t('updates.summaryUnavailable')}</div>
			) : (
				<div className="card updates-summary">
					<div className="updates-summary__head">
						{summary.running ? (
							<>
								<Spinner />
								<span className="updates-summary__status">{t('updates.statusRunning')}</span>
							</>
						) : (
							<span className="updates-summary__status updates-summary__status--idle">
								{summary.lastRunAt
									? t('updates.lastRun', { time: relativeTime(summary.lastRunAt) })
									: t('updates.idle')}
							</span>
						)}
					</div>
					<div className="updates-summary__counts">
						<div className="updates-stat">
							<div className="updates-stat__v">{summary.total}</div>
							<div className="updates-stat__k">{t('updates.total')}</div>
						</div>
						<div className="updates-stat updates-stat--ok">
							<div className="updates-stat__v">{summary.complete}</div>
							<div className="updates-stat__k">{t('updates.complete')}</div>
						</div>
						<div className="updates-stat updates-stat--bad">
							<div className="updates-stat__v">{summary.failed}</div>
							<div className="updates-stat__k">{t('updates.failed')}</div>
						</div>
						<div className="updates-stat">
							<div className="updates-stat__v">{summary.skipped}</div>
							<div className="updates-stat__k">{t('updates.skipped')}</div>
						</div>
						<div className="updates-stat">
							<div className="updates-stat__v">{summary.pending}</div>
							<div className="updates-stat__k">{t('updates.pending')}</div>
						</div>
					</div>
					<div className="updates-summary__ops">
						<span
							className={online ? 'ws-dot ws-dot--on' : 'ws-dot'}
							title={online ? t('updates.wsOk') : t('updates.wsDown')}
						/>
						<Button
							variant="primary"
							loading={updating}
							disabled={running}
							onClick={() => void runUpdate()}
						>
							{t('updates.runNow')}
						</Button>
						{cats.length > 0 && (
							<>
								<Select
									aria-label={t('updates.selectCategory')}
									value={catSel}
									onChange={setCatSel}
								>
									<option value="">{t('updates.allCategories')}</option>
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
									{t('updates.runByCategory')}
								</Button>
							</>
						)}
					</div>
				</div>
			)}

			{jobs.length > 0 && (
				<div className="card updates-jobs">
					<div className="faint updates-jobs__title">{t('updates.liveFeed')}</div>
					{jobs.map((j) => (
						<div key={j.seq} className="updates-job">
							<span className={`updates-job__dot updates-job__dot--${j.status}`} />
							<span className="updates-job__title">{j.mangaTitle}</span>
							<span className={`updates-job__status updates-job__status--${j.status}`}>
								{JOB_LABEL_KEY[j.status] ? t(JOB_LABEL_KEY[j.status]) : j.status}
							</span>
							{j.addedChapters > 0 && (
								<span className="updates-job__added">
									{t('updates.addedChapters', { count: j.addedChapters })}
								</span>
							)}
						</div>
					))}
				</div>
			)}

			<h2 className="updates-recent__heading">{t('updates.recentHeading')}</h2>
			{recent.isPending ? (
				<div className="updates-list">
					{Array.from({ length: 4 }, (_, i) => (
						<Skeleton key={i} height={76} />
					))}
				</div>
			) : recentEntries.length === 0 ? (
				<div className="updates-empty faint">{t('updates.recentEmpty')}</div>
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
										<Badge>{t('updates.newChapter')}</Badge>
									</div>
									<div className="faint updates-item__time">
										{relativeTime(e.fetchedAt)}
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
