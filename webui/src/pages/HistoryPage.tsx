import { useMemo } from 'react';
import { Link } from 'react-router-dom';
import { Clock } from 'lucide-react';
import { coverUrl } from '../api/client';
import { useHistory } from '../api/hooks';
import { chapterLabel, relativeTime } from '../lib/format';
import { Button, Cover, EmptyState, InfiniteList, PageHeader, Skeleton } from '../components';
import './history.css';

export default function HistoryPage() {
	const query = useHistory();
	const entries = useMemo(
		() => query.data?.pages.flatMap((p) => p.entries) ?? [],
		[query.data]
	);

	return (
		<div className="page container">
			<PageHeader title="历史" />

			{query.isPending ? (
				<div className="history-list">
					{Array.from({ length: 6 }, (_, i) => (
						<Skeleton key={i} height={76} />
					))}
				</div>
			) : entries.length === 0 ? (
				<EmptyState
					icon={Clock}
					title="还没有阅读记录"
					hint="看过的章节会出现在这里，方便随时续读"
					action={
						<Link to="/library">
							<Button variant="primary">去书库</Button>
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
								to={`/reader/${h.chapter.id}?manga=${h.manga.id}&page=${h.lastPageRead}`}
							>
								<div className="history-item__thumb">
									<Cover src={coverUrl(h.manga.id)} title={h.manga.title} />
								</div>
								<div className="history-item__meta">
									<div className="history-item__title">{h.manga.title}</div>
									<div className="muted history-item__chapter">{chapterLabel(h.chapter)}</div>
								</div>
								<div className="history-item__right">
									<span className="faint">{relativeTime(h.lastReadAt)}</span>
									{h.isRead ? (
										<span className="faint">已读完</span>
									) : (
										<span className="history-item__progress">读到 P.{h.lastPageRead + 1}</span>
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
