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
	searchTracker,
	trackerKeys
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
import { Badge, Button, Cover, Input, Modal, SegmentedControl, Select, Spinner } from '../../components';
import { TrackerLogo } from '../../components/trackers/TrackerLogo';
import './metadata.css';

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
	const [provider, setProvider] = useState<TrackerProvider>('anilist');
	const [draft, setDraft] = useState(mangaTitle);
	const [submitted, setSubmitted] = useState('');
	const [selection, setSelection] = useState<Selection | null>(null);
	const [checked, setChecked] = useState<Partial<Record<MetadataField, boolean>>>({});
	const [titleValue, setTitleValue] = useState('');
	const [bind, setBind] = useState(false);
	const [applied, setApplied] = useState<MetadataField[] | null>(null);

	const linksQuery = useQuery({
		queryKey: trackerKeys.mangaLinks(mangaId),
		queryFn: () => listTrackerLinks(mangaId)
	});
	const links = linksQuery.data ?? [];

	const sourcesQuery = useSources();
	const sources = useMemo(
		() => (sourcesQuery.data ?? []).filter((source) => source.enabled).sort(compareSourceName),
		[sourcesQuery.data]
	);
	const sourceName = useMemo(() => {
		const byId = new Map(sources.map((source) => [source.id, source.name]));
		return (id: number) => byId.get(id) ?? `#${id}`;
	}, [sources]);
	const sourceIds = useMemo(() => sources.map((source) => source.id), [sources]);
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
	const sourceSearch = useGlobalSearch(sourceIds, submitted, tab === 'source' && submitted.length > 0);

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
		setBind(false);
	}, [previewQuery.data]);

	const selectedFields = useMemo(
		() => (previewQuery.data?.fields ?? []).filter((field) => checked[field.field]).map((field) => field.field),
		[previewQuery.data, checked]
	);

	const apply = useMutation({
		mutationFn: () => {
			const values: Partial<Record<MetadataField, string | Array<string> | null>> = {};
			const title = previewQuery.data?.fields.find((field) => field.field === 'title');
			if (title && titleValue && titleValue !== title.incoming) values.title = titleValue;
			return applyMetadata(mangaId, {
				provider: wireProvider(selection!),
				remoteId: selection!.remoteId,
				fields: selectedFields,
				values,
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
											onChecked={(value) =>
												setChecked((cur) => ({ ...cur, [field.field]: value }))
											}
											onTitleValue={setTitleValue}
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
	onChecked,
	onTitleValue
}: {
	field: MetadataPreviewField;
	sourceName: (key: string) => string | undefined;
	checked: boolean;
	titleValue: string;
	onChecked: (value: boolean) => void;
	onTitleValue: (value: string) => void;
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
					) : (
						displayValue(field.incoming)
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
