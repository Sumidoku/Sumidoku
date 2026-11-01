import { useEffect, useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { ExternalLink, Search, TriangleAlert } from 'lucide-react';
import { sourceImageUrl } from '../../api/client';
import { useGlobalSearch, useSources } from '../../api/hooks';
import { applyMetadata, invalidateMangaMetadata, lockProviderLabel, previewMetadata } from '../../api/metadata';
import {
	TRACKER_PROVIDERS,
	TRACKER_PROVIDER_NAMES,
	invalidateTrackerLinks,
	isTrackerAuthError,
	listTrackerLinks,
	preferredTrackerProvider,
	searchTracker,
	trackerKeys,
	trackerPreferences
} from '../../api/trackers';
import type {
	GlobalSearchResult,
	MetadataField,
	MetadataPreviewField,
	SearchManga,
	TrackSearchItem,
	TrackerProvider
} from '../../api/types';
import { compareSourceName } from '../../lib/source';
import {
	CONTENT_RATING_LEVELS,
	MULTI_LANGUAGE,
	compareLanguageLabel,
	isMultiLanguage,
	languageLabel
} from '../../lib/source';
import {
	type GlobalSearchFilterPrefs,
	type TriState,
	loadGlobalSearchFilters,
	saveGlobalSearchFilters
} from '../../lib/globalSearchPrefs';
import { Badge, Button, Cover, Input, Modal, SegmentedControl, Select, Spinner, TriStateDropdown } from '../../components';
import { TrackerLogo } from '../../components/trackers/TrackerLogo';
import './metadata.css';

type FilterGroup = 'ratings' | 'langs' | 'sources';

type Selection =
	| {
			kind: 'tracker';
			provider: TrackerProvider;
			remoteId: string;
			title: string;
			coverUrl?: string | null;
	  }
	| {
			kind: 'source';
			sourceId: number;
			remoteId: string;
			title: string;
			cover?: string | null;
	  };

function wireProvider(selection: Selection): string {
	return selection.kind === 'tracker' ? `tracker:${selection.provider}` : `source:${selection.sourceId}`;
}

function selectionLabel(selection: Selection, sourceName: (id: number) => string): string {
	return selection.kind === 'tracker'
		? TRACKER_PROVIDER_NAMES[selection.provider]
		: sourceName(selection.sourceId);
}

function displayValue(value: string | Array<string> | null): string {
	if (value === null) return '—';
	return Array.isArray(value) ? value.join(', ') : value;
}

/** The aggregate search returns each source's page separately; flatten it into one candidate list */
function sourceCandidates(results: GlobalSearchResult[] | undefined): Array<{ sourceId: number; entries: SearchManga[] }> {
	return (results ?? [])
		.filter((result) => !result.error)
		.map((result) => ({ sourceId: result.sourceId, entries: result.entries }))
		.filter((group) => group.entries.length > 0);
}

export function MatchMetadataDialog({
	mangaId,
	mangaTitle,
	onClose
}: {
	mangaId: number;
	mangaTitle: string;
	onClose: () => void;
}) {
	const { t } = useTranslation();
	const qc = useQueryClient();
	const [tab, setTab] = useState<'tracker' | 'source'>('tracker');
	const preferencesQuery = useQuery({ queryKey: trackerKeys.preferences, queryFn: trackerPreferences });
	const [picked, setProvider] = useState<TrackerProvider | null>(null);
	const provider = picked ?? preferredTrackerProvider(preferencesQuery.data?.defaultTracker, TRACKER_PROVIDERS);
	const [draft, setDraft] = useState(mangaTitle);
	const [submitted, setSubmitted] = useState('');
	const [selection, setSelection] = useState<Selection | null>(null);
	const [checked, setChecked] = useState<Partial<Record<MetadataField, boolean>>>({});
	const [titleValue, setTitleValue] = useState('');
	const [values, setValues] = useState<Partial<Record<MetadataField, string>>>({});
	const [bind, setBind] = useState(false);
	const [applied, setApplied] = useState<MetadataField[] | null>(null);

	const linksQuery = useQuery({
		queryKey: trackerKeys.mangaLinks(mangaId),
		queryFn: () => listTrackerLinks(mangaId)
	});
	const links = linksQuery.data ?? [];

	const sourcesQuery = useSources();
	const [filters, setFilters] = useState<GlobalSearchFilterPrefs>(loadGlobalSearchFilters);
	useEffect(() => saveGlobalSearchFilters(filters), [filters]);
	const sources = useMemo(
		() => (sourcesQuery.data ?? []).filter((source) => source.enabled).sort(compareSourceName),
		[sourcesQuery.data]
	);
	const sourceName = useMemo(() => {
		const byId = new Map(sources.map((source) => [source.id, source.name]));
		return (id: number) => byId.get(id) ?? `#${id}`;
	}, [sources]);
	const langOptions = useMemo(() => {
		const set = new Set<string>();
		for (const s of sources) {
			for (const l of s.languages) {
				const code = l.trim().toLowerCase();
				if (code && !isMultiLanguage(code)) set.add(code);
			}
		}
		return [MULTI_LANGUAGE, ...[...set].sort(compareLanguageLabel)];
	}, [sources]);

	function sourceMatches(s: { contentRating: number; languages: string[]; id: number }, f: GlobalSearchFilterPrefs): boolean {
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
		return (
			triAllows(f.ratings, [String(s.contentRating)]) &&
			triAllows(f.langs, s.languages.map((l) => l.trim().toLowerCase())) &&
			triAllows(f.sources, [String(s.id)])
		);
	}

	const selectedSources = useMemo(() => sources.filter((s) => sourceMatches(s, filters)), [sources, filters]);
	const selectedSourceIds = useMemo(() => selectedSources.map((s) => s.id), [selectedSources]);
	// Locks carry the source key, so the label resolves by key rather than by instance id
	const lockSourceName = useMemo(() => {
		const byKey = new Map(sources.map((source) => [source.key, source.name]));
		return (key: string) => byKey.get(key);
	}, [sources]);

	const trackerQuery = useQuery({
		queryKey: trackerKeys.search(provider, submitted),
		queryFn: () => searchTracker(provider, submitted),
		enabled: tab === 'tracker' && submitted.length > 0,
		retry: false
	});
	const sourceSearch = useGlobalSearch(selectedSourceIds, submitted, tab === 'source' && submitted.length > 0);

	const previewQuery = useQuery({
		queryKey: ['metadata', 'preview', mangaId, selection ? wireProvider(selection) : '', selection?.remoteId ?? ''],
		queryFn: () => previewMetadata(mangaId, wireProvider(selection!), selection!.remoteId),
		enabled: selection !== null,
		retry: false
	});

	// Every selectable field is proposed, the rest stay off until the user picks them
	useEffect(() => {
		const preview = previewQuery.data;
		if (!preview) return;
		const next: Partial<Record<MetadataField, boolean>> = {};
		for (const field of preview.fields) next[field.field] = field.selectable;
		setChecked(next);
		const title = preview.fields.find((field) => field.field === 'title');
		setTitleValue(typeof title?.incoming === 'string' ? title.incoming : '');
		setValues({});
		setBind(false);
	}, [previewQuery.data]);

	const selectedFields = useMemo(
		() => (previewQuery.data?.fields ?? []).filter((field) => checked[field.field]).map((field) => field.field),
		[previewQuery.data, checked]
	);

	const apply = useMutation({
		mutationFn: () => {
			const valuesPayload: Partial<Record<MetadataField, string | Array<string> | null>> = {};
			const title = previewQuery.data?.fields.find((field) => field.field === 'title');
			if (title && titleValue && titleValue !== title.incoming) valuesPayload.title = titleValue;
			for (const field of previewQuery.data?.fields ?? []) {
				if (field.field === 'title') continue;
				if (!field.selectable || !checked[field.field]) continue;
				const v = values[field.field];
				if (v === undefined || v === '') continue;
				if (Array.isArray(field.incoming)) {
					valuesPayload[field.field] = v
						.split(',')
						.map((s) => s.trim())
						.filter((s) => s.length > 0);
				} else {
					valuesPayload[field.field] = v;
				}
			}
			return applyMetadata(mangaId, {
				provider: wireProvider(selection!),
				remoteId: selection!.remoteId,
				fields: selectedFields,
				values: Object.keys(valuesPayload).length > 0 ? valuesPayload : undefined,
				bind
			});
		},
		onSuccess: (result) => {
			invalidateMangaMetadata(qc, mangaId);
			invalidateTrackerLinks(qc, mangaId);
			setApplied(result.appliedFields);
		}
	});

	function submitSearch(event?: FormEvent) {
		event?.preventDefault();
		const name = draft.trim();
		if (!name) return;
		if (name === submitted) {
			void trackerQuery.refetch();
			void sourceSearch.refetch();
		} else {
			setSubmitted(name);
		}
	}

	function cycle(group: FilterGroup, key: string) {
		setFilters((cur) => {
			const rec = { ...cur[group] };
			const curVal = (rec[key] ?? 0) as number;
			const next = curVal === 0 ? 1 : curVal === 1 ? -1 : 0;
			if (next === 0) delete rec[key];
			else rec[key] = next as TriState;
			return { ...cur, [group]: rec };
		});
	}
	function resetGroup(group: FilterGroup) {
		setFilters((cur) => ({ ...cur, [group]: {} }));
	}

	function choose(next: Selection) {
		setSelection(next);
		setApplied(null);
	}

	const step = applied !== null ? 'applied' : selection !== null ? 'preview' : 'search';

	return (
		<Modal
			open
			title={t('metadata.title')}
			onClose={onClose}
			panelClassName="modal__panel--wide"
			footer={
				step === 'preview' ? (
					<>
						<Button onClick={() => setSelection(null)}>{t('metadata.backToSearch')}</Button>
						<Button
							variant="primary"
							disabled={selectedFields.length === 0}
							loading={apply.isPending}
							onClick={() => apply.mutate()}
						>
							{t('metadata.applySelected', { count: selectedFields.length })}
						</Button>
					</>
				) : step === 'applied' ? (
					<Button variant="primary" onClick={onClose}>
						{t('metadata.done')}
					</Button>
				) : (
					<Button onClick={onClose}>{t('common.cancel')}</Button>
				)
			}
		>
			{step === 'search' && (
				<div className="metadata-dialog">
					{links.length > 0 && (
						<section className="metadata-linked">
							<h4>{t('metadata.linkedTrackers')}</h4>
							<ul>
								{links.map((link) => (
									<li key={`${link.provider}:${link.trackId}`}>
										<TrackerLogo provider={link.provider} />
										<span>{TRACKER_PROVIDER_NAMES[link.provider]}</span>
										<span className="muted">{link.title ?? link.trackId}</span>
										<Button
											size="sm"
											onClick={() =>
												choose({
													kind: 'tracker',
													provider: link.provider,
													remoteId: link.trackId,
													title: link.title ?? link.trackId
												})
											}
										>
											{t('metadata.pullMetadata')}
										</Button>
									</li>
								))}
							</ul>
						</section>
					)}

					<SegmentedControl
						value={tab}
						options={[
							{ value: 'tracker', label: t('metadata.tabTracker') },
							{ value: 'source', label: t('metadata.tabSource') }
						]}
						onChange={(value) => {
							setTab(value);
							setSubmitted('');
						}}
					/>

					{tab === 'tracker' && (
						<SegmentedControl
							value={provider}
							options={TRACKER_PROVIDERS.map((item) => ({
								value: item,
								label: (
									<span className="tracker-seg__option">
										<TrackerLogo provider={item} />
										<span>{TRACKER_PROVIDER_NAMES[item]}</span>
									</span>
								)
							}))}
							onChange={(value) => {
								setProvider(value);
								setSubmitted('');
							}}
						/>
					)}

					{tab === 'source' && (
						<div className="filter-header">
							<TriStateDropdown label={t('search.rating')} options={CONTENT_RATING_LEVELS.map((l) => ({ key: String(l.value), label: l.label }))} states={filters.ratings} onCycle={(k) => cycle('ratings', k)} onReset={() => resetGroup('ratings')} />
							<TriStateDropdown label={t('search.language')} options={langOptions.map((c) => ({ key: c, label: languageLabel(c) }))} states={filters.langs} onCycle={(k) => cycle('langs', k)} onReset={() => resetGroup('langs')} />
							<TriStateDropdown label={t('search.source')} options={sources.map((s) => ({ key: String(s.id), label: s.name }))} states={filters.sources} onCycle={(k) => cycle('sources', k)} onReset={() => resetGroup('sources')} />
						</div>
					)}

					<form className="tracker-search" onSubmit={submitSearch}>
						<Input
							icon={<Search size={15} />}
							type="search"
							placeholder={t('metadata.searchPlaceholder')}
							value={draft}
							aria-label={t('common.search')}
							onChange={(event) => setDraft(event.currentTarget.value)}
						/>
						<Button type="submit" variant="primary" loading={submitted.length > 0 && (tab === 'tracker' ? trackerQuery.isFetching : sourceSearch.isFetching)}>
							{t('common.search')}
						</Button>
					</form>

					{tab === 'tracker' ? (
						submitted.length === 0 ? null : trackerQuery.isError ? (
							isTrackerAuthError(trackerQuery.error) ? (
								<p className="tracker-auth" role="alert">
									{t('trackers.authRequired', { provider: TRACKER_PROVIDER_NAMES[provider] })}
								</p>
							) : (
								<div className="tracker-error" role="alert">
									<span>{trackerQuery.error.message}</span>
								</div>
							)
						) : trackerQuery.isPending ? (
							<div className="tracker-dialog-state">
								<Spinner />
								{t('metadata.searching')}
							</div>
						) : trackerQuery.data.length === 0 ? (
							<p className="muted">{t('metadata.noResults')}</p>
						) : (
							<ul className="tracker-results">
								{trackerQuery.data.map((item) => (
									<TrackerCandidate
										key={item.id}
										item={item}
										tracked={links.some(
											(link) => link.provider === provider && link.trackId === item.id
										)}
										onChoose={() =>
											choose({
												kind: 'tracker',
												provider,
												remoteId: item.id,
												title: item.title,
												coverUrl: item.coverUrl
											})
										}
									/>
								))}
							</ul>
						)
					) : submitted.length === 0 ? null : sourceSearch.isError ? (
						<div className="tracker-error" role="alert">
							<TriangleAlert size={14} />
							<span>{sourceSearch.error?.message}</span>
						</div>
					) : sourceCandidates(sourceSearch.data?.sources).length === 0 ? (
						<p className="muted">{sourceSearch.isFetching ? t('metadata.searching') : t('metadata.noResults')}</p>
					) : (
						sourceCandidates(sourceSearch.data?.sources).map((group) => (
							<section key={group.sourceId} className="metadata-source-group">
								<h4>{sourceName(group.sourceId)}</h4>
								<ul className="tracker-results">
									{group.entries.map((entry) => (
										<li className="tracker-result" key={entry.key}>
											<div className="tracker-result__cover">
												<Cover
													src={entry.cover ? sourceImageUrl(group.sourceId, entry.cover) : entry.cover}
													title={entry.title}
												/>
											</div>
											<div className="tracker-result__meta">
												<h4>{entry.title}</h4>
											</div>
											<Button
												size="sm"
												onClick={() =>
													choose({
														kind: 'source',
														sourceId: group.sourceId,
														remoteId: entry.key,
														title: entry.title,
														cover: entry.cover
													})
												}
											>
												{t('metadata.useThis')}
											</Button>
										</li>
									))}
								</ul>
							</section>
						))
					)}
				</div>
			)}

			{step === 'preview' && selection !== null && (
				<div className="metadata-dialog">
					<header className="metadata-candidate">
						<div className="metadata-candidate__cover">
							<Cover
								src={
									selection.kind === 'tracker'
										? selection.coverUrl
										: selection.cover
											? sourceImageUrl(selection.sourceId, selection.cover)
											: selection.cover
								}
								title={selection.title}
							/>
						</div>
						<div>
							<h4>{previewQuery.data?.candidate.title ?? selection.title}</h4>
							<p className="muted">
								{t('metadata.matchFrom', { source: selectionLabel(selection, sourceName) })}
							</p>
							{previewQuery.data?.candidate.url && (
								<a href={previewQuery.data.candidate.url} target="_blank" rel="noreferrer">
									<ExternalLink size={13} /> {t('metadata.openMatch')}
								</a>
							)}
						</div>
					</header>

					{previewQuery.isError ? (
						<div className="tracker-error" role="alert">
							<span>{previewQuery.error.message}</span>
						</div>
					) : previewQuery.isPending ? (
						<div className="tracker-dialog-state">
							<Spinner />
							{t('common.loading')}
						</div>
					) : previewQuery.data ? (
						<>
							<table className="metadata-fields">
								<thead>
									<tr>
										<th />
										<th>{t('metadata.columnField')}</th>
										<th>{t('metadata.columnCurrent')}</th>
										<th>{t('metadata.columnIncoming')}</th>
									</tr>
								</thead>
								<tbody>
									{previewQuery.data.fields.map((field) => (
										<FieldRow
											key={field.field}
											field={field}
											sourceName={lockSourceName}
											checked={Boolean(checked[field.field])}
											titleValue={titleValue}
											values={values}
											onChecked={(value) =>
												setChecked((cur) => ({ ...cur, [field.field]: value }))
											}
											onTitleValue={setTitleValue}
											onValue={(f, v) => setValues((cur) => ({ ...cur, [f]: v }))}
										/>
									))}
								</tbody>
							</table>

							{selection.kind === 'tracker' && (
								<label className="metadata-bind">
									{previewQuery.data.linked ? (
										<span className="muted">{t('metadata.alreadyBound')}</span>
									) : (
										<>
											<input
												type="checkbox"
												checked={bind}
												onChange={(event) => setBind(event.currentTarget.checked)}
											/>
											<span>{t('metadata.bindTracker', { provider: TRACKER_PROVIDER_NAMES[selection.provider] })}</span>
										</>
									)}
								</label>
							)}

							{apply.isError && (
								<p className="tracker-error" role="alert">
									{apply.error.message}
								</p>
							)}
						</>
					) : null}
				</div>
			)}

			{step === 'applied' && applied !== null && (
				<div className="metadata-dialog metadata-applied">
					<p>{t('metadata.appliedSummary', { count: applied.length })}</p>
					<p className="muted">{t('metadata.appliedHint')}</p>
					{applied.length > 0 && (
						<ul className="metadata-applied__fields">
							{applied.map((field) => (
								<li key={field}>{t(`metadata.fieldLabels.${field}`)}</li>
							))}
						</ul>
					)}
				</div>
			)}
		</Modal>
	);
}

function TrackerCandidate({
	item,
	tracked,
	onChoose
}: {
	item: TrackSearchItem;
	tracked: boolean;
	onChoose: () => void;
}) {
	const { t } = useTranslation();
	return (
		<li className="tracker-result">
			<div className="tracker-result__cover">
				<Cover src={item.coverUrl} title={item.title} />
			</div>
			<div className="tracker-result__meta">
				<h4>{item.title}</h4>
				{item.info && <p className="tracker-result__info">{item.info}</p>}
				{item.description && (
					<p className="tracker-result__description" title={item.description}>
						{item.description}
					</p>
				)}
				<div className="tracker-result__badges">{tracked && <Badge>{t('trackers.tracked')}</Badge>}</div>
			</div>
			<Button size="sm" onClick={onChoose}>
				{t('metadata.useThis')}
			</Button>
		</li>
	);
}

function FieldRow({
	field,
	sourceName,
	checked,
	titleValue,
	values,
	onChecked,
	onTitleValue,
	onValue
}: {
	field: MetadataPreviewField;
	sourceName: (key: string) => string | undefined;
	checked: boolean;
	titleValue: string;
	values: Partial<Record<MetadataField, string>>;
	onChecked: (value: boolean) => void;
	onTitleValue: (value: string) => void;
	onValue: (field: MetadataField, value: string) => void;
}) {
	const { t } = useTranslation();
	const value = field.current;
	const multi = Array.isArray(value);
	return (
		<tr className={field.selectable ? undefined : 'metadata-fields__row--disabled'}>
			<td>
				<input
					type="checkbox"
					checked={checked}
					disabled={!field.selectable}
					aria-label={field.label}
					onChange={(event) => onChecked(event.currentTarget.checked)}
				/>
			</td>
			<td>
				<span>{t(`metadata.fieldLabels.${field.field}`)}</span>
				{field.lock && (
					<span className="metadata-fields__lock">
						{t('metadata.lockedBy', {
							provider:
								field.lock.provider === 'manual'
									? t('metadata.manualLock')
									: lockProviderLabel(field.lock.provider, sourceName)
						})}
					</span>
				)}
			</td>
			<td>
				{field.field === 'cover' ? (
					<CoverCell url={coverUrlOf(value)} label={t('metadata.fieldLabels.cover')} />
				) : multi ? (
					value.join(', ')
				) : (
					displayValue(value)
				)}
			</td>
			<td>
				{field.field === 'title' && field.alternatives.length > 0 ? (
					<Select value={titleValue || String(field.incoming)} onChange={onTitleValue}>
						{field.alternatives.map((alternative) => (
							<option value={alternative.value} key={`${alternative.label}:${alternative.value}`}>
								{alternative.label} · {alternative.value}
							</option>
						))}
					</Select>
				) : field.selectable ? (
					field.field === 'cover' ? (
						<CoverCell url={coverUrlOf(field.incoming)} label={t('metadata.fieldLabels.cover')} />
					) : Array.isArray(field.incoming) ? (
						<Input
							value={values[field.field] ?? (field.incoming as string[]).join(', ')}
							onChange={(e) => onValue(field.field, e.currentTarget.value)}
						/>
					) : (
						<Input
							value={values[field.field] ?? String(field.incoming ?? '')}
							onChange={(e) => onValue(field.field, e.currentTarget.value)}
						/>
					)
				) : (
					<span className="muted">{field.reason ?? t('metadata.noIncomingValue')}</span>
				)}
			</td>
		</tr>
	);
}

/// The cover is the only field whose value is an image address: show the two candidate images instead
function coverUrlOf(value: unknown): string | null {
	return typeof value === 'string' && value.length > 0 ? value : null;
}

function CoverCell({ url, label }: { url: string | null; label: string }) {
	if (!url) return <span className="muted">—</span>;
	return (
		<div className="metadata-fields__cover">
			<Cover src={url} title={label} />
		</div>
	);
}

export type { Selection as MetadataSelection };
