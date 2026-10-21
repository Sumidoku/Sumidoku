import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { Search, SearchX, TriangleAlert } from 'lucide-react';
import { sourceImageUrl } from '../api/client';
import { useGlobalSearch, useRegisterManga, useSources } from '../api/hooks';
import type { Manga, Source, SourceSearchResult } from '../api/types';
import { statusText } from '../lib/format';
import {
	type GlobalSearchFilterPrefs,
	type TriState,
	loadGlobalSearchFilters,
	saveGlobalSearchFilters
} from '../lib/globalSearchPrefs';
import {
	CONTENT_RATING_LEVELS,
	MULTI_LANGUAGE,
	compareLanguageLabel,
	compareSourceName,
	isMultiLanguage,
	languageLabel
} from '../lib/source';
import { Button, CoverCard, EmptyState, Input, Skeleton, TriStateDropdown } from '../components';

type FilterGroup = 'ratings' | 'langs' | 'sources';

/** Next state of the tri-state cycle: unselected → include → exclude → unselected */
function nextTri(cur: TriState | 0): TriState | 0 {
	return cur === 0 ? 1 : cur === 1 ? -1 : 0;
}

/** Per-dimension tri-state verdict: with any include selected the entry must hit one of them; hitting any exclude knocks it out */
function triAllows(states: Record<string, TriState>, keys: string[]): boolean {
	let includes = 0;
	let includeHit = false;
	for (const [k, v] of Object.entries(states)) {
		if (v === 1) {
			includes += 1;
			if (keys.includes(k)) includeHit = true;
		} else if (keys.includes(k)) {
			return false;
		}
	}
	return includes === 0 || includeHit;
}

function sourceMatches(s: Source, f: GlobalSearchFilterPrefs): boolean {
	return (
		triAllows(f.ratings, [String(s.contentRating)]) &&
		triAllows(
			f.langs,
			s.languages.map((l) => l.trim().toLowerCase())
		) &&
		triAllows(f.sources, [String(s.id)])
	);
}

export default function SearchPage() {
	const navigate = useNavigate();
	const { t } = useTranslation();
	const sourcesQuery = useSources();
	const [filters, setFilters] = useState<GlobalSearchFilterPrefs>(loadGlobalSearchFilters);
	useEffect(() => saveGlobalSearchFilters(filters), [filters]);

	// 500ms input debounce, same as in-source search
	const [input, setInput] = useState('');
	const [query, setQuery] = useState('');
	useEffect(() => {
		const timer = setTimeout(() => setQuery(input.trim()), 500);
		return () => clearTimeout(timer);
	}, [input]);

	// Disabled sources neither participate nor appear as filter candidates; display and candidate order follow source name (same convention as the browse page)
	const sources = useMemo(
		() => (sourcesQuery.data ?? []).filter((s) => s.enabled).sort(compareSourceName),
		[sourcesQuery.data]
	);
	const langOptions = useMemo(() => {
		const set = new Set<string>();
		for (const s of sources) {
			for (const l of s.languages) {
				const code = l.trim().toLowerCase();
				if (code && !isMultiLanguage(code)) set.add(code);
			}
		}
		// Same as Aidoku: the multi-language sentinel always sits at the top of the list
		return [MULTI_LANGUAGE, ...[...set].sort(compareLanguageLabel)];
	}, [sources]);

	const selected = useMemo(() => sources.filter((s) => sourceMatches(s, filters)), [sources, filters]);
	const sourceIds = useMemo(() => selected.map((s) => s.id), [selected]);

	const search = useGlobalSearch(sourceIds, query, query !== '' && sourceIds.length > 0);
	const resultsById = useMemo(() => {
		const map = new Map<number, SourceSearchResult>();
		for (const r of search.data?.sources ?? []) map.set(r.sourceId, r);
		return map;
	}, [search.data]);

	const register = useRegisterManga();
	// The busy state uses a `<sourceId>:<key>` composite key: different sources can collide on the same key, and a bare key would cross-mark them
	const [pendingKey, setPendingKey] = useState<string | null>(null);

	async function openEntry(m: Manga) {
		if (m.id) {
			navigate(`/manga/${m.id}`);
			return;
		}
		// Every entry of an aggregate result carries its own sourceId, so registration must land on its owning source
		const owner = m.sourceId;
		if (owner === undefined || pendingKey) return;
		setPendingKey(`${owner}:${m.key}`);
		try {
			const { id } = await register.mutateAsync({ sourceId: owner, manga: m });
			navigate(`/manga/${id}`);
		} catch {
			// client already popped an error toast
		} finally {
			setPendingKey(null);
		}
	}

	function busyKey(m: Manga): string {
		return `${m.sourceId}:${m.key}`;
	}

	function cycle(group: FilterGroup, key: string) {
		setFilters((cur) => {
			const rec = { ...cur[group] };
			const next = nextTri(rec[key] ?? 0);
			if (next === 0) delete rec[key];
			else rec[key] = next;
			return { ...cur, [group]: rec };
		});
	}

	// A source with no results takes no section (other sources are still streaming in, so empty shells would flood the screen);
	// only when everything has finished without a single result does the global empty state show
	const visible = selected.filter((s) => {
		const r = resultsById.get(s.id);
		return !r || r.error !== undefined || r.entries.length > 0;
	});

	return (
		<div className="page container">
			<form
				className="source-page__search"
				onSubmit={(e) => {
					e.preventDefault();
					setQuery(input.trim());
				}}
			>
				<Input
					icon={<Search size={15} />}
					type="search"
					placeholder={t('search.placeholder')}
					value={input}
					onChange={(e) => setInput(e.currentTarget.value)}
				/>
			</form>

			<div className="filter-header">
				<TriStateDropdown
					label={t('search.rating')}
					options={CONTENT_RATING_LEVELS.map((l) => ({ key: String(l.value), label: l.label }))}
					states={filters.ratings}
					onCycle={(k) => cycle('ratings', k)}
				/>
				<TriStateDropdown
					label={t('search.language')}
					options={langOptions.map((c) => ({ key: c, label: languageLabel(c) }))}
					states={filters.langs}
					onCycle={(k) => cycle('langs', k)}
				/>
				<TriStateDropdown
					label={t('search.source')}
					options={sources.map((s) => ({ key: String(s.id), label: s.name }))}
					states={filters.sources}
					onCycle={(k) => cycle('sources', k)}
				/>
			</div>

			{query === '' ? (
				<EmptyState icon={Search} title={t('search.emptyTitle')} hint={t('search.emptyHint')} />
			) : selected.length === 0 ? (
				<EmptyState
					icon={SearchX}
					title={t('search.noSourcesTitle')}
					hint={t('search.noSourcesHint')}
				/>
			) : search.isError ? (
				<EmptyState
					icon={TriangleAlert}
					title={t('search.loadFailedTitle')}
					hint={search.error?.message}
					action={
						<Button variant="primary" onClick={() => void search.refetch()}>
							{t('search.retry')}
						</Button>
					}
				/>
			) : visible.length === 0 && !search.isFetching ? (
				<EmptyState
					icon={SearchX}
					title={t('search.noResultsTitle')}
					hint={t('search.noResultsHint')}
				/>
			) : (
				visible.map((s) => (
					<SourceSection
						key={s.id}
						source={s}
						result={resultsById.get(s.id)}
						pendingKey={pendingKey}
						busyKey={busyKey}
						onOpen={(m) => void openEntry(m)}
						onViewAll={() => navigate(`/browse/${s.id}?q=${encodeURIComponent(query)}`)}
					/>
				))
			)}
		</div>
	);
}

function SourceSection({
	source,
	result,
	pendingKey,
	busyKey,
	onOpen,
	onViewAll
}: {
	source: Source;
	result: SourceSearchResult | undefined;
	pendingKey: string | null;
	busyKey: (m: Manga) => string;
	onOpen: (m: Manga) => void;
	onViewAll: () => void;
}) {
	const { t } = useTranslation();
	return (
		<section className="home-section">
			<header className="home-section__head">
				<div className="home-section__titles">
					<h2 className="home-section__title">{source.name}</h2>
				</div>
				<Button size="sm" onClick={onViewAll}>
					{t('search.viewAll')}
				</Button>
			</header>
			{!result ? (
				<div className="home-hrow">
					{Array.from({ length: 5 }, (_, i) => (
						<Skeleton key={i} width={120} aspect="2 / 3" style={{ borderRadius: 'var(--r-md)' }} />
					))}
				</div>
			) : result.error ? (
				<p className="search-page__error">{result.error.message}</p>
			) : (
				<div className="home-hrow">
					{result.entries.map((m) => (
						<div className="home-scroller__item" key={m.key}>
							<CoverCard
								src={m.cover ? sourceImageUrl(source.id, m.cover) : m.cover}
								title={m.title}
								subtitle={m.authors?.[0] ?? statusText(m.status)}
								busy={pendingKey === busyKey(m)}
								onClick={() => onOpen(m)}
							/>
						</div>
					))}
				</div>
			)}
		</section>
	);
}
