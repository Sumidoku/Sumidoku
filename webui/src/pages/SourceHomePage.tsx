import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { ArrowLeft, ChevronRight, House, Search, TriangleAlert } from 'lucide-react';
import { sourceImageUrl } from '../api/client';
import { useRegisterManga, useSource, useSourceHome } from '../api/hooks';
import type {
	AidokuListing,
	HomeComponent,
	HomeComponentValue,
	HomeFilterItem,
	Link,
	Listing,
	MangaWithChapter,
	SourceManga
} from '../api/types';
import { chapterLabel, relativeTime } from '../lib/format';
import { Button, Chip, Cover, CoverCard, EmptyState, IconButton, Skeleton } from '../components';

export default function SourceHomePage() {
	const { t } = useTranslation();
	const { sourceId: rawId } = useParams();
	const sourceId = Number(rawId);
	const navigate = useNavigate();

	const source = useSource(sourceId);
	const homeQuery = useSourceHome(sourceId, source.data?.enabled ?? true);

	const register = useRegisterManga();
	const [pendingKey, setPendingKey] = useState<string | null>(null);

	async function openManga(m: SourceManga) {
		if (pendingKey) return;
		setPendingKey(m.key);
		try {
			const { id } = await register.mutateAsync({ sourceId, manga: m });
			navigate(`/manga/${id}`);
		} catch {
			// client already popped an error toast
		} finally {
			setPendingKey(null);
		}
	}

	function openListing(listing: Listing | AidokuListing) {
		navigate(`/browse/${sourceId}?mode=${encodeURIComponent(listing.id)}`);
	}

	function openLink(link: Link) {
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

	function linkBusy(link: Link): boolean {
		return link.value !== null && 'Manga' in link.value && pendingKey === link.value.Manga.key;
	}

	/** Only the server can attach the anti-hotlink headers a source declares, so covers all go through the server proxy */
	function proxyImage(url: string | null): string | null {
		return url ? sourceImageUrl(sourceId, url) : url;
	}

	if (source.data && !source.data.enabled) {
		return (
			<div className="page container">
				<div className="source-page__head">
					<IconButton label={t('sourceHome.backToBrowse')} onClick={() => navigate('/browse')}>
						<ArrowLeft size={18} />
					</IconButton>
					<h1 className="source-page__title">{source.data.name}</h1>
				</div>
				<EmptyState icon={TriangleAlert} title={t('sourceHome.sourceDisabled')} hint={t('sourceHome.sourceDisabledHint')} />
			</div>
		);
	}

	const home = homeQuery.data;
	// During streaming, partials are already written to the cache: render edge-to-edge as soon as there is data; skeletons are only for when there is nothing at all
	const showSkeleton = (!home && homeQuery.isPending) || (home !== null && home !== undefined && home.components.length === 0 && homeQuery.isFetching);
	// layout events arrive first (component slots carry real titles with empty payloads); component events fill the slots by title;
	// during streaming an unfilled slot renders the Aidoku redacted+shimmering style skeleton, swapped for the real thing once it lands
	const streaming = homeQuery.isFetching;

	return (
		<div className="page container">
			<div className="source-page__head">
				<IconButton
					label={t('sourceHome.backToSource', { name: source.data?.name ?? t('sourceHome.sourceFallback') })}
					onClick={() => navigate(`/browse/${sourceId}`)}
				>
					<ArrowLeft size={18} />
				</IconButton>
				<h1 className="source-page__title">{source.data?.name ?? t('sourceHome.sourceFallback')}</h1>
				<Button className="source-page__actions" onClick={() => navigate(`/browse/${sourceId}`)}>
					<Search size={15} />
					{t('common.search')}
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
					title={t('sourceHome.loadFailed')}
					hint={homeQuery.error?.message}
					action={
						<Button variant="primary" onClick={() => void homeQuery.refetch()}>
							{t('sourceHome.retry')}
						</Button>
					}
				/>
			) : home === null || home === undefined ? (
				<EmptyState
					icon={House}
					title={t('sourceHome.noHome')}
					hint={t('sourceHome.noHomeHint')}
					action={
						<Button variant="primary" onClick={() => navigate(`/browse/${sourceId}`)}>
							{t('sourceHome.goSearch')}
						</Button>
					}
				/>
			) : home.components.length === 0 ? (
				<EmptyState icon={House} title={t('sourceHome.emptyHome')} hint={t('sourceHome.emptyHomeHint')} />
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
			const renderRow = (link: Link, i: number) => {
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
			// page_size is horizontal-pagination semantics: each page stacks page_size entries vertically into a column, and columns are swiped through sideways
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
		// Unknown component kind: keep the section title, skip the body
		return null;
	}
}

/** Whether a component is still unfilled during streaming: the layout slot's payload entry array is empty in every variant */
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

/** Typed skeleton for components that have not arrived (Aidoku redacted+shimmering style placeholder); the title is rendered by HomeSection from the slot's real title */
function PendingBody({ c }: { c: HomeComponent }) {
	const v = c.value;
	// List shapes: row skeletons
	if ('MangaList' in v || 'MangaChapterList' in v || 'Links' in v) {
		return (
			<div className="home-rows">
				{[0, 1, 2].map((i) => (
					<Skeleton key={i} height={64} style={{ borderRadius: 'var(--r-md)' }} />
				))}
			</div>
		);
	}
	// Filter chips: chip skeletons
	if ('Filters' in v) {
		return (
			<div className="home-chips">
				{[0, 1, 2, 3].map((i) => (
					<Skeleton key={i} width={72} height={26} style={{ borderRadius: 999 }} />
				))}
			</div>
		);
	}
	// Banner carousel: lay out banner skeletons at the declared aspect ratio
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
	// Scroller / BigScroller: horizontal cover skeletons
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
	onOpenListing: (listing: Listing | AidokuListing) => void;
	renderBody: () => ReactNode;
}) {
	const { t } = useTranslation();
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
							{t('sourceHome.viewAll')}
						</Button>
					)}
				</header>
			)}
			{renderBody()}
		</section>
	);
}

/** ImageScroller body: title/subtitle overlaid at the banner bottom, auto_scroll_interval drives the auto sideways scrolling */
function HomeImageScroller({
	value,
	onOpenLink
}: {
	value: Extract<HomeComponentValue, { ImageScroller: unknown }>['ImageScroller'];
	onOpenLink: (link: Link) => void;
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
 * Horizontal auto-scrolling driven by auto_scroll_interval (seconds): advance to the next item each tick,
 * wrapping around at the end. Paused while the pointer hovers, so it never fights the user's own swiping
 */
function useAutoScroll(interval: number | null, itemCount: number) {
	const rowRef = useRef<HTMLDivElement | null>(null);
	const [paused, setPaused] = useState(false);
	useEffect(() => {
		if (!interval || interval <= 0 || itemCount < 2 || paused) return;
		const id = window.setInterval(() => {
			const el = rowRef.current;
			if (!el) return;
			// a child's offsetLeft is relative to .home-hrow (position:relative), the same coordinate system as scrollLeft
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
