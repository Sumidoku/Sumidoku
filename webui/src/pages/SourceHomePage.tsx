import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { ArrowLeft, ChevronRight, House, Search, TriangleAlert } from 'lucide-react';
import { sourceImageUrl } from '../api/client';
import { useRegisterManga, useSource, useSourceHome } from '../api/hooks';
import type {
	HomeComponent,
	HomeComponentValue,
	HomeFilterItem,
	HomeLink,
	Listing,
	Manga,
	MangaWithChapter
} from '../api/types';
import { chapterLabel, relativeTime } from '../lib/format';
import { Button, Chip, Cover, CoverCard, EmptyState, IconButton, Skeleton } from '../components';

export default function SourceHomePage() {
	const { sourceId: rawId } = useParams();
	const sourceId = Number(rawId);
	const navigate = useNavigate();

	const source = useSource(sourceId);
	const homeQuery = useSourceHome(sourceId, source.data?.enabled ?? true);

	const register = useRegisterManga();
	const [pendingKey, setPendingKey] = useState<string | null>(null);

	async function openManga(m: Manga) {
		if (m.id) {
			navigate(`/manga/${m.id}`);
			return;
		}
		if (pendingKey) return;
		setPendingKey(m.key);
		try {
			const { id } = await register.mutateAsync({ sourceId, manga: m });
			navigate(`/manga/${id}`);
		} catch {
			// client 已弹错误 toast
		} finally {
			setPendingKey(null);
		}
	}

	function openListing(listing: Listing) {
		navigate(`/browse/${sourceId}?mode=${encodeURIComponent(listing.id)}`);
	}

	function openLink(link: HomeLink) {
		const v = link.value;
		if (!v) return;
		if ('Url' in v) {
			window.open(v.Url, '_blank', 'noopener,noreferrer');
		} else if ('Listing' in v) {
			openListing(v.Listing);
		} else {
			void openManga(v.Manga);
		}
	}

	function openFilterItem(item: HomeFilterItem) {
		const qs = new URLSearchParams({ mode: 'search' });
		if (item.values && item.values.length > 0) {
			qs.set('filters', JSON.stringify(item.values));
		}
		navigate(`/browse/${sourceId}?${qs}`);
	}

	function linkBusy(link: HomeLink): boolean {
		return link.value !== null && 'Manga' in link.value && pendingKey === link.value.Manga.key;
	}

	/** 防盗链要的头只有服务端能按源声明带上，封面统一走服务端代理 */
	function proxyImage(url: string | null): string | null {
		return url ? sourceImageUrl(sourceId, url) : url;
	}

	if (source.data && !source.data.enabled) {
		return (
			<div className="page container">
				<div className="source-page__head">
					<IconButton label="返回浏览" onClick={() => navigate('/browse')}>
						<ArrowLeft size={18} />
					</IconButton>
					<h1 className="source-page__title">{source.data.name}</h1>
				</div>
				<EmptyState icon={TriangleAlert} title="该源已停用" hint="重新安装或启用后才能浏览" />
			</div>
		);
	}

	const home = homeQuery.data;
	// 流式加载中 partial 已写进缓存：有数据就边到边渲染，骨架只留给全无数据时
	const showSkeleton = (!home && homeQuery.isPending) || (home !== null && home !== undefined && home.components.length === 0 && homeQuery.isFetching);
	// layout 事件先到（组件槽位带真实标题、payload 为空），component 事件按 title 替换填实；
	// 流式期间未填实的槽位Aidoku redacted+shimmering 同款渲染骨架，到位即换真身
	const streaming = homeQuery.isFetching;

	return (
		<div className="page container">
			<div className="source-page__head">
				<IconButton label={`返回 ${source.data?.name ?? '源'}`} onClick={() => navigate(`/browse/${sourceId}`)}>
					<ArrowLeft size={18} />
				</IconButton>
				<h1 className="source-page__title">{source.data?.name ?? '源'}</h1>
				<Button className="source-page__actions" onClick={() => navigate(`/browse/${sourceId}`)}>
					<Search size={15} />
					搜索
				</Button>
			</div>

			{showSkeleton ? (
				<>
					{[0, 1].map((i) => (
						<div className="home-section" key={i}>
							<Skeleton height={20} width={140} style={{ marginBottom: 10 }} />
							<div className="home-hrow">
								{Array.from({ length: 5 }, (_, j) => (
									<Skeleton key={j} width={120} aspect="2 / 3" style={{ borderRadius: 'var(--r-md)' }} />
								))}
							</div>
						</div>
					))}
				</>
			) : homeQuery.isError ? (
				<EmptyState
					icon={TriangleAlert}
					title="加载失败"
					hint={homeQuery.error?.message}
					action={
						<Button variant="primary" onClick={() => void homeQuery.refetch()}>
							重试
						</Button>
					}
				/>
			) : home === null || home === undefined ? (
				<EmptyState
					icon={House}
					title="该源没有主页"
					hint="这个源没有提供主页内容，可以直接搜索或按列表浏览"
					action={
						<Button variant="primary" onClick={() => navigate(`/browse/${sourceId}`)}>
							去搜索
						</Button>
					}
				/>
			) : home.components.length === 0 ? (
				<EmptyState icon={House} title="主页没有内容" hint="源返回了一个空的主页布局" />
			) : (
				home.components.map((c, i) => (
					<HomeSection
						key={i}
						component={c}
						onOpenListing={openListing}
						renderBody={() => (streaming && isPendingComponent(c) ? <PendingBody c={c} /> : renderBody(c))}
					/>
				))
			)}
		</div>
	);

	function renderBody(c: HomeComponent) {
		const v = c.value;
		if ('ImageScroller' in v) {
			return <HomeImageScroller value={v.ImageScroller} onOpenLink={openLink} />;
		}
		if ('BigScroller' in v) {
			return (
				<div className="home-hrow">
					{v.BigScroller.entries.map((m) => (
						<button
							key={m.key}
							type="button"
							className="home-big-card"
							disabled={pendingKey === m.key}
							onClick={() => void openManga(m)}
						>
							<span className="home-big-card__cover">
								<Cover src={proxyImage(m.cover)} title={m.title} />
								{(m.content_rating === 'NSFW' || m.content_rating === 'Suggestive') && (
									<span
										className={`home-big-card__rating${m.content_rating === 'NSFW' ? ' home-big-card__rating--nsfw' : ''}`}
									>
										{m.content_rating}
									</span>
								)}
							</span>
							<span className="home-big-card__main">
								<span className="home-big-card__title">{m.title}</span>
								{m.authors && m.authors.length > 0 && (
									<span className="home-big-card__author">{m.authors.join(', ')}</span>
								)}
								{m.description && <span className="home-big-card__desc">{m.description}</span>}
								{m.tags && m.tags.length > 0 && <span className="home-big-card__tags">{m.tags.join(' · ')}</span>}
							</span>
						</button>
					))}
				</div>
			);
		}
		if ('Scroller' in v) {
			return (
				<div className="home-hrow">
					{v.Scroller.entries.map((link, i) => (
						<div className="home-scroller__item" key={link.title + i}>
							<CoverCard
								src={proxyImage(link.image_url)}
								title={link.title}
								subtitle={link.subtitle}
								busy={linkBusy(link)}
								onClick={link.value ? () => openLink(link) : undefined}
							/>
						</div>
					))}
				</div>
			);
		}
		if ('MangaList' in v) {
			const { ranking, page_size, entries } = v.MangaList;
			const renderRow = (link: HomeLink, i: number) => {
				const tags = link.value && 'Manga' in link.value ? link.value.Manga.tags : null;
				return (
					<button
						key={link.title + i}
						type="button"
						className="home-row"
						disabled={linkBusy(link)}
						onClick={() => openLink(link)}
					>
						{ranking && <span className="home-row__rank">{i + 1}</span>}
						<Cover src={proxyImage(link.image_url)} title={link.title} />
						<span className="home-row__main">
							<span className="home-row__title">{link.title}</span>
							{link.subtitle && <span className="home-row__subtitle">{link.subtitle}</span>}
							{tags && tags.length > 0 && <span className="home-row__tags">{tags.join(' · ')}</span>}
						</span>
						<ChevronRight size={16} className="home-row__chevron" />
					</button>
				);
			};
			// page_size 是横向分页语义：每页 page_size 条竖排成一列，列间横滑翻页
			if (page_size && page_size > 0) {
				return (
					<div className="home-hrow home-pages">
						{chunk(entries, page_size).map((page, pi) => (
							<div className="home-rows home-page" key={pi}>
								{page.map((link, i) => renderRow(link, pi * page_size + i))}
							</div>
						))}
					</div>
				);
			}
			return <div className="home-rows">{entries.map(renderRow)}</div>;
		}
		if ('MangaChapterList' in v) {
			const { page_size, entries } = v.MangaChapterList;
			const renderRow = ({ manga, chapter }: MangaWithChapter) => (
				<button
					key={manga.key + chapter.key}
					type="button"
					className="home-row"
					disabled={pendingKey === manga.key}
					onClick={() => void openManga(manga)}
				>
					<Cover src={proxyImage(manga.cover)} title={manga.title} />
					<span className="home-row__main">
						<span className="home-row__title">{manga.title}</span>
						<span className="home-row__subtitle">
							{chapterLabel(chapter)}
							{relativeTime(chapter.date_uploaded) && ` · ${relativeTime(chapter.date_uploaded)}`}
						</span>
					</span>
					<ChevronRight size={16} className="home-row__chevron" />
				</button>
			);
			if (page_size && page_size > 0) {
				return (
					<div className="home-hrow home-pages">
						{chunk(entries, page_size).map((page, pi) => (
							<div className="home-rows home-page" key={pi}>
								{page.map(renderRow)}
							</div>
						))}
					</div>
				);
			}
			return <div className="home-rows">{entries.map(renderRow)}</div>;
		}
		if ('Filters' in v) {
			return (
				<div className="home-chips">
					{v.Filters.map((item, i) => (
						<Chip key={item.title + i} onClick={() => openFilterItem(item)}>
							{item.title}
						</Chip>
					))}
				</div>
			);
		}
		if ('Links' in v) {
			return (
				<div className="home-rows">
					{v.Links.map((link, i) => (
						<button
							key={link.title + i}
							type="button"
							className="home-row home-row--link"
							disabled={linkBusy(link)}
							onClick={() => openLink(link)}
						>
							<span className="home-row__main">
								<span className="home-row__title">{link.title}</span>
								{link.subtitle && <span className="home-row__subtitle">{link.subtitle}</span>}
							</span>
							<ChevronRight size={16} className="home-row__chevron" />
						</button>
					))}
				</div>
			);
		}
		// 未知组件种类：保留区块标题，正文跳过
		return null;
	}
}

/** 流式期间组件还没填实的判定：layout 槽位的 payload 各变体条目数组为空 */
function isPendingComponent(c: HomeComponent): boolean {
	const v = c.value;
	if ('ImageScroller' in v) return v.ImageScroller.links.length === 0;
	if ('BigScroller' in v) return v.BigScroller.entries.length === 0;
	if ('Scroller' in v) return v.Scroller.entries.length === 0;
	if ('MangaList' in v) return v.MangaList.entries.length === 0;
	if ('MangaChapterList' in v) return v.MangaChapterList.entries.length === 0;
	if ('Filters' in v) return v.Filters.length === 0;
	return v.Links.length === 0;
}

/** 未到组件的类型化骨架（Aidoku redacted+shimmering 同款占位）；标题由 HomeSection 用槽位真实标题渲染 */
function PendingBody({ c }: { c: HomeComponent }) {
	const v = c.value;
	// 列表形：行骨架
	if ('MangaList' in v || 'MangaChapterList' in v || 'Links' in v) {
		return (
			<div className="home-rows">
				{[0, 1, 2].map((i) => (
					<Skeleton key={i} height={64} style={{ borderRadius: 'var(--r-md)' }} />
				))}
			</div>
		);
	}
	// 筛选 chips：chip 骨架
	if ('Filters' in v) {
		return (
			<div className="home-chips">
				{[0, 1, 2, 3].map((i) => (
					<Skeleton key={i} width={72} height={26} style={{ borderRadius: 999 }} />
				))}
			</div>
		);
	}
	// 横幅轮播：按声明的宽高比铺横幅骨架
	if ('ImageScroller' in v) {
		const { width, height } = v.ImageScroller;
		const ratio = width && height && width > 0 && height > 0 ? `${width} / ${height}` : '16 / 9';
		return (
			<div className="home-hrow">
				{[0, 1].map((i) => (
					<Skeleton key={i} width={280} aspect={ratio} style={{ borderRadius: 'var(--r-md)' }} />
				))}
			</div>
		);
	}
	// Scroller / BigScroller：封面横排骨架
	return (
		<div className="home-hrow">
			{Array.from({ length: 5 }, (_, i) => (
				<Skeleton key={i} width={120} aspect="2 / 3" style={{ borderRadius: 'var(--r-md)' }} />
			))}
		</div>
	);
}

function HomeSection({
	component,
	onOpenListing,
	renderBody
}: {
	component: HomeComponent;
	onOpenListing: (listing: Listing) => void;
	renderBody: () => ReactNode;
}) {
	const v = component.value;
	const listing =
		('Scroller' in v && v.Scroller.listing) ||
		('MangaList' in v && v.MangaList.listing) ||
		('MangaChapterList' in v && v.MangaChapterList.listing) ||
		null;
	return (
		<section className="home-section">
			{(component.title || component.subtitle || listing) && (
				<header className="home-section__head">
					<div className="home-section__titles">
						{component.title && <h2 className="home-section__title">{component.title}</h2>}
						{component.subtitle && <p className="home-section__subtitle">{component.subtitle}</p>}
					</div>
					{listing && (
						<Button size="sm" onClick={() => onOpenListing(listing)}>
							查看全部
						</Button>
					)}
				</header>
			)}
			{renderBody()}
		</section>
	);
}

/** ImageScroller 正文：横幅底部叠标题/副标题，auto_scroll_interval 驱动自动横滚 */
function HomeImageScroller({
	value,
	onOpenLink
}: {
	value: Extract<HomeComponentValue, { ImageScroller: unknown }>['ImageScroller'];
	onOpenLink: (link: HomeLink) => void;
}) {
	const { links, width, height, auto_scroll_interval } = value;
	const ratio = width && height && width > 0 && height > 0 ? `${width} / ${height}` : undefined;
	const { rowRef, pauseProps } = useAutoScroll(auto_scroll_interval, links.length);
	return (
		<div className="home-hrow" ref={rowRef} {...pauseProps}>
			{links.map((link, i) => (
				<button
					key={i}
					type="button"
					className="home-banner"
					style={ratio ? { aspectRatio: ratio } : undefined}
					onClick={() => onOpenLink(link)}
				>
					{link.image_url ? (
						<>
							<img src={link.image_url} alt={link.title} loading="lazy" referrerPolicy="no-referrer" />
							<span className="home-banner__caption">
								<span className="home-banner__caption-title">{link.title}</span>
								{link.subtitle && <span className="home-banner__caption-sub">{link.subtitle}</span>}
							</span>
						</>
					) : (
						<span className="home-banner__fallback">{link.title}</span>
					)}
				</button>
			))}
		</div>
	);
}

/**
 * auto_scroll_interval（秒）驱动的横向自动滚动：每次前进到下一项，到头回卷。
 * 指针悬停时暂停，避免和用户的手动滑动打架
 */
function useAutoScroll(interval: number | null, itemCount: number) {
	const rowRef = useRef<HTMLDivElement | null>(null);
	const [paused, setPaused] = useState(false);
	useEffect(() => {
		if (!interval || interval <= 0 || itemCount < 2 || paused) return;
		const id = window.setInterval(() => {
			const el = rowRef.current;
			if (!el) return;
			// 子项 offsetLeft 以 .home-hrow（position:relative）为参照，与 scrollLeft 同坐标系
			const next = Array.from(el.children).find((c) => (c as HTMLElement).offsetLeft > el.scrollLeft + 8);
			el.scrollTo({ left: next ? (next as HTMLElement).offsetLeft : 0, behavior: 'smooth' });
		}, interval * 1000);
		return () => window.clearInterval(id);
	}, [interval, itemCount, paused]);
	return {
		rowRef,
		pauseProps: {
			onPointerEnter: () => setPaused(true),
			onPointerLeave: () => setPaused(false)
		}
	};
}

function chunk<T>(items: T[], size: number): T[][] {
	const out: T[][] = [];
	for (let i = 0; i < items.length; i += size) {
		out.push(items.slice(i, i + size));
	}
	return out;
}
