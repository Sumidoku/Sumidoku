import { useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { ArrowLeft, ChevronRight, House, Search, TriangleAlert } from 'lucide-react';
import { useRegisterManga, useSource, useSourceHome } from '../api/hooks';
import type { HomeComponent, HomeFilterItem, HomeLink, Listing, Manga } from '../api/types';
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
						renderBody={() => renderBody(c)}
					/>
				))
			)}
		</div>
	);

	function renderBody(c: HomeComponent) {
		const v = c.value;
		if ('ImageScroller' in v) {
			const { links, width, height } = v.ImageScroller;
			const ratio = width && height && width > 0 && height > 0 ? `${width} / ${height}` : undefined;
			return (
				<div className="home-hrow">
					{links.map((link, i) => (
						<button
							key={i}
							type="button"
							className="home-banner"
							style={ratio ? { aspectRatio: ratio } : undefined}
							onClick={() => openLink(link)}
						>
							{link.image_url ? (
								<img src={link.image_url} alt={link.title} loading="lazy" referrerPolicy="no-referrer" />
							) : (
								<span className="home-banner__fallback">{link.title}</span>
							)}
						</button>
					))}
				</div>
			);
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
							<Cover src={m.cover} title={m.title} />
							<span className="home-big-card__main">
								<span className="home-big-card__title">{m.title}</span>
								{m.authors && m.authors.length > 0 && (
									<span className="home-big-card__author">{m.authors.join(', ')}</span>
								)}
								{m.description && <span className="home-big-card__desc">{m.description}</span>}
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
								src={link.image_url}
								title={link.title}
								busy={linkBusy(link)}
								onClick={link.value ? () => openLink(link) : undefined}
							/>
						</div>
					))}
				</div>
			);
		}
		if ('MangaList' in v) {
			const { ranking, entries } = v.MangaList;
			return (
				<div className="home-rows">
					{entries.map((link, i) => (
						<button
							key={link.title + i}
							type="button"
							className="home-row"
							disabled={linkBusy(link)}
							onClick={() => openLink(link)}
						>
							{ranking && <span className="home-row__rank">{i + 1}</span>}
							<Cover src={link.image_url} title={link.title} />
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
		if ('MangaChapterList' in v) {
			return (
				<div className="home-rows">
					{v.MangaChapterList.entries.map(({ manga, chapter }) => (
						<button
							key={manga.key + chapter.key}
							type="button"
							className="home-row"
							disabled={pendingKey === manga.key}
							onClick={() => void openManga(manga)}
						>
							<Cover src={manga.cover} title={manga.title} />
							<span className="home-row__main">
								<span className="home-row__title">{manga.title}</span>
								<span className="home-row__subtitle">
									{chapterLabel(chapter)}
									{relativeTime(chapter.date_uploaded) && ` · ${relativeTime(chapter.date_uploaded)}`}
								</span>
							</span>
							<ChevronRight size={16} className="home-row__chevron" />
						</button>
					))}
				</div>
			);
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
