import { Bookmark, Check, CheckCircle2, Download, FileDown, Lock } from 'lucide-react';
import { chapterFileUrl } from '../../api/client';
import type { Chapter } from '../../api/types';
import { chapterLabel, formatDate } from '../../lib/format';
import { IconButton, Spinner } from '../../components';

/** idle 可点下载；pending 入队请求中；queued 已入队（下载进度去下载页看） */
export type ChapterDlState = 'idle' | 'pending' | 'queued';

interface ChapterRowProps {
	chapter: Chapter;
	selecting: boolean;
	selected: boolean;
	/** 普通模式进阅读，选择模式切选中 */
	onActivate: () => void;
	dlState: ChapterDlState;
	onDownload: () => void;
	/** 本地漫画的文件本来就在盘上，不渲染下载入口 */
	hideDownload?: boolean;
	onToggleBookmark: () => void;
	bookmarkPending: boolean;
}

export function ChapterRow({
	chapter: c,
	selecting,
	selected,
	onActivate,
	dlState,
	onDownload,
	hideDownload,
	onToggleBookmark,
	bookmarkPending
}: ChapterRowProps) {
	const cls = [
		'chapter-row',
		c.isRead ? 'chapter-row--read' : '',
		selecting && selected ? 'chapter-row--selected' : ''
	]
		.filter(Boolean)
		.join(' ');
	const uploaded = formatDate(c.date_uploaded);
	// wire 上 scanlators 可能是 null（aidoku Option<Vec>），不是空数组
	const scanlator = c.scanlators?.[0] ?? '';

	return (
		<div
			className={cls}
			role="button"
			tabIndex={0}
			onClick={onActivate}
			onKeyDown={(e) => {
				if (e.key === 'Enter' || e.key === ' ') {
					e.preventDefault();
					onActivate();
				}
			}}
		>
			{selecting && (
				<span className="chapter-row__check">
					{selected && <Check size={12} strokeWidth={3} />}
				</span>
			)}
			<span className="chapter-row__num">{c.chapter_number ?? '—'}</span>
			<span className="chapter-row__main">
				<span className="chapter-row__title">{chapterLabel(c)}</span>
				{(uploaded || scanlator) && (
					<span className="chapter-row__meta">
						{uploaded && <span>{uploaded}</span>}
						{scanlator && <span>{scanlator}</span>}
					</span>
				)}
			</span>
			<span className="chapter-row__status">
				{c.locked && (
					<span className="chapter-row__lock" title="锁定">
						<Lock size={14} />
					</span>
				)}
				{c.isBookmarked && (
					<span className="chapter-row__bm" title="书签">
						<Bookmark size={14} fill="currentColor" />
					</span>
				)}
				{c.isDownloaded && (
					<span className="chapter-row__dl-ok" title="已下载">
						<CheckCircle2 size={14} />
					</span>
				)}
				{!c.isRead && c.lastPageRead > 0 && (
					<span className="chapter-row__prog">P.{c.lastPageRead + 1}</span>
				)}
			</span>
			{!selecting && (
				<span className="chapter-row__actions">
					{!hideDownload &&
						(c.isDownloaded ? (
						<a
							className="chapter-row__cbz"
							href={chapterFileUrl(c.id)}
							download
							title="下载 CBZ 文件"
							aria-label="下载 CBZ 文件"
							onClick={(e) => e.stopPropagation()}
						>
							<FileDown size={16} />
						</a>
					) : dlState === 'idle' ? (
						<IconButton
							label="下载本章"
							onClick={(e) => {
								e.stopPropagation();
								onDownload();
							}}
						>
							<Download size={16} />
						</IconButton>
					) : (
						<IconButton label={dlState === 'pending' ? '正在加入下载队列' : '已在下载队列中'} disabled>
							<Spinner size={15} />
						</IconButton>
					))}
					<IconButton
						label={c.isBookmarked ? '取消书签' : '加书签'}
						disabled={bookmarkPending}
						onClick={(e) => {
							e.stopPropagation();
							onToggleBookmark();
						}}
					>
						<Bookmark size={16} fill={c.isBookmarked ? 'currentColor' : 'none'} />
					</IconButton>
				</span>
			)}
		</div>
	);
}
