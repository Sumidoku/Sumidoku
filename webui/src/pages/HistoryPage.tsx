import { useMemo } from 'react';
import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { Clock } from 'lucide-react';
import { coverUrl } from '../api/client';
import { useHistory } from '../api/hooks';
import { chapterLabel, relativeTime } from '../lib/format';
import { Button, Cover, EmptyState, InfiniteList, PageHeader, Skeleton } from '../components';
import './history.css';

export default function HistoryPage() {
	const { t } = useTranslation();
	const query = useHistory();
	const entries = useMemo(
		() => query.data?.pages.flatMap((p) => p.entries) ?? [],
		[query.data]
	);

	return (
		<div className="page container">
			<PageHeader title={t('common.nav.history')} />

			{query.isPending ? (
				<div className="history-list">
					{Array.from({ length: 6 }, (_, i) => (
						<Skeleton key={i} height={76} />
					))}
				</div>
			) : entries.length === 0 ? (
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
						{entries.map((h) => (
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
