import { useMemo } from 'react';
import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { Clock } from 'lucide-react';
import { coverUrl } from '../api/client';
import { useHistory } from '../api/hooks';
import type { HistoryEntry } from '../api/types';
import { chapterLabel, relativeTime } from '../lib/format';
import { Button, Cover, EmptyState, InfiniteList, PageHeader, Skeleton } from '../components';
import './history.css';

interface HistoryRow {
	entry: HistoryEntry;
	extra: number;
}

export default function HistoryPage() {
	const { t } = useTranslation();
	const query = useHistory();
	const rows = useMemo<HistoryRow[]>(() => {
		const all = query.data?.pages.flatMap((p) => p.entries) ?? [];
		// Group by manga + calendar day, keep the latest entry and count how many were collapsed
		const groups = new Map<string, HistoryRow>();
		for (const e of all) {
			const d = new Date(e.lastReadAt * 1000);
			const key = `${e.manga.id}@${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
			const cur = groups.get(key);
			if (!cur) {
				groups.set(key, { entry: e, extra: 0 });
			} else {
				cur.extra += 1;
				if (e.lastReadAt > cur.entry.lastReadAt) {
					cur.entry = e;
				}
			}
		}
		return [...groups.values()].sort((a, b) => b.entry.lastReadAt - a.entry.lastReadAt);
	}, [query.data]);

	return (
		<div className="page container">
			<PageHeader title={t('common.nav.history')} />

			{query.isPending ? (
				<div className="history-list">
					{Array.from({ length: 6 }, (_, i) => (
						<Skeleton key={i} height={76} />
					))}
				</div>
			) : rows.length === 0 ? (
				<EmptyState
					icon={Clock}
					title={t('history.emptyTitle')}
					hint={t('history.emptyHint')}
					action={
						<Link to="/library">
							<Button variant="primary">{t('history.goLibrary')}</Button>
						</Link>
					}
				/>
			) : (
				<InfiniteList
					hasNextPage={query.hasNextPage}
					isFetching={query.isFetching}
					onLoadMore={() => void query.fetchNextPage()}
				>
					<div className="history-list">
						{rows.map(({ entry: h, extra }) => (
							<Link
								key={h.chapter.id}
								className="history-item card"
								to={`/manga/${h.manga.id}`}
							>
								<div className="history-item__thumb">
									<Cover src={coverUrl(h.manga.id, h.manga.cover)} title={h.manga.title} />
								</div>
								<div className="history-item__meta">
									<div className="history-item__title">{h.manga.title}</div>
									<div className="muted history-item__chapter">{chapterLabel(h.chapter)}</div>
									{extra > 0 && (
										<div className="faint history-item__extra">
											{t('history.more', { count: extra })}
										</div>
									)}
								</div>
								<div className="history-item__right">
									<span className="faint">{relativeTime(h.lastReadAt)}</span>
									{h.isRead ? (
										<span className="faint">{t('history.finished')}</span>
									) : (
										<span className="history-item__progress">
											{t('history.progress', { page: h.lastPageRead + 1 })}
										</span>
									)}
								</div>
							</Link>
						))}
					</div>
				</InfiniteList>
			)}
		</div>
	);
}
