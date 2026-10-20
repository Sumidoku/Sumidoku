import { Bookmark, Check, CheckCircle2, Download, FileDown, Lock, MinusCircle } from 'lucide-react';
import { chapterFileUrl } from '../../api/client';
import type { Chapter } from '../../api/types';
import { chapterLabel, formatDate } from '../../lib/format';
import { IconButton, MenuButton, Spinner } from '../../components';

/** idle = downloadable; pending = enqueue request in flight; queued = enqueued (watch download progress on the downloads page) */
export type ChapterDlState = 'idle' | 'pending' | 'queued';

interface ChapterRowProps {
	chapter: Chapter;
	selecting: boolean;
	selected: boolean;
	/** Normal mode enters reading, select mode toggles selection */
	onActivate: () => void;
	dlState: ChapterDlState;
	onDownload: () => void;
	/** A local manga's files are already on disk, so no download entry is rendered */
	hideDownload?: boolean;
	onToggleBookmark: () => void;
	bookmarkPending: boolean;
	// Aidoku's per-chapter read marking (MangaView context menu); entries hide by their own conditions
	onMarkRead?: () => void;
	onMarkUnread?: () => void;
	/** Mark-previous targets the rows below this one in the visible list, so it hides on the last row (Aidoku: !last) */
	onMarkPreviousRead?: () => void;
	onMarkPreviousUnread?: () => void;
	markPending?: boolean;
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
	bookmarkPending,
	onMarkRead,
	onMarkUnread,
	onMarkPreviousRead,
	onMarkPreviousUnread,
	markPending
}: ChapterRowProps) {
	const cls = [
		'chapter-row',
		c.isRead ? 'chapter-row--read' : '',
		selecting && selected ? 'chapter-row--selected' : ''
	]
		.filter(Boolean)
		.join(' ');
	const uploaded = formatDate(c.date_uploaded);
	// On the wire scanlators can be null (aidoku Option<Vec>), not an empty array
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
				{(onMarkRead || onMarkUnread || onMarkPreviousRead) && (
					<MenuButton label="章节操作">
						{(close) => (
							<>
								{/* Aidoku MARK_READ: offered while the chapter is not completed */}
								{onMarkRead && !c.isRead && (
									<button
										type="button"
										className="menu-row"
										disabled={markPending}
										onClick={() => {
											close();
											onMarkRead();
										}}
									>
										<span>标记已读</span>
										<CheckCircle2 size={14} />
									</button>
								)}
								{/* Aidoku MARK_UNREAD: offered while the chapter carries any reading trace */}
								{onMarkUnread && (c.isRead || c.lastPageRead > 0 || (c.lastReadAt ?? 0) > 0) && (
									<button
										type="button"
										className="menu-row"
										disabled={markPending}
										onClick={() => {
											close();
											onMarkUnread();
										}}
									>
										<span>标记未读</span>
										<MinusCircle size={14} />
									</button>
								)}
								{/* Aidoku MARK_PREVIOUS: the parent withholds these on the last visible row */}
								{onMarkPreviousRead && (
									<button
										type="button"
										className="menu-row"
										disabled={markPending}
										onClick={() => {
											close();
											onMarkPreviousRead();
										}}
									>
										<span>标记上一章为已读</span>
										<CheckCircle2 size={14} />
									</button>
								)}
								{onMarkPreviousUnread && (
									<button
										type="button"
										className="menu-row"
										disabled={markPending}
										onClick={() => {
											close();
											onMarkPreviousUnread();
										}}
									>
										<span>标记上一章为未读</span>
										<MinusCircle size={14} />
									</button>
								)}
							</>
						)}
					</MenuButton>
				)}
				</span>
			)}
		</div>
	);
}
