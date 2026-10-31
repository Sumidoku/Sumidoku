import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { LockOpen, RotateCcw } from 'lucide-react';
import { lockProviderLabel, restoreMetadata } from '../../api/metadata';
import { useManga, usePatchManga, useSources } from '../../api/hooks';
import type { ContentRating, Manga, MangaStatus, MetadataField } from '../../api/types';
import { Button, Input, Modal, Select, Spinner } from '../../components';
import './metadata.css';

const EDITABLE: Array<keyof FormState> = [
	'title',
	'authors',
	'artists',
	'description',
	'tags',
	'status',
	'contentRating'
];

const STATUSES: Array<MangaStatus> = ['Unknown', 'Ongoing', 'Completed', 'Cancelled', 'Hiatus'];
const RATINGS: Array<ContentRating> = ['Unknown', 'Safe', 'Suggestive', 'NSFW'];

const joinList = (values: Array<string> | null | undefined) => (values ?? []).join(', ');
const splitList = (value: string) =>
	value
		.split(',')
		.map((item) => item.trim())
		.filter(Boolean);

function sameList(a: Array<string>, b: Array<string>) {
	return a.length === b.length && a.every((item, index) => item === b[index]);
}

interface FormState {
	title: string;
	authors: string;
	artists: string;
	description: string;
	tags: string;
	status: MangaStatus;
	contentRating: ContentRating;
}

function formFrom(manga: Manga): FormState {
	return {
		title: manga.title,
		authors: joinList(manga.authors),
		artists: joinList(manga.artists),
		description: manga.description ?? '',
		tags: joinList(manga.tags),
		status: manga.status,
		contentRating: manga.content_rating
	};
}

/** Manual metadata editing: saved values get a "manual" lock, so source refreshes keep them */
export function EditMetadataDialog({ mangaId, onClose }: { mangaId: number; onClose: () => void }) {
	const { t } = useTranslation();
	const mangaQuery = useManga(mangaId);
	const sourcesQuery = useSources();
	const manga = mangaQuery.data;
	const [form, setForm] = useState<FormState | null>(null);
	const [restoring, setRestoring] = useState(false);
	const formRef = useRef<HTMLFormElement>(null);

	// Seed the draft once; later refetches must not clobber what the user is typing
	useEffect(() => {
		if (manga) setForm((current) => current ?? formFrom(manga));
	}, [manga]);

	const sourceName = (id: number) =>
		(sourcesQuery.data ?? []).find((source) => source.id === id)?.name;
	const lockOf = (field: MetadataField) => manga?.metadataLocks?.[field];

	const patch = usePatchManga();
	const patchBody = (next: FormState): Parameters<typeof patch.mutate>[0] => {
		const body: Parameters<typeof patch.mutate>[0] = { id: mangaId };
		if (!manga) return body;
		const authors = splitList(next.authors);
		const artists = splitList(next.artists);
		const tags = splitList(next.tags);
		if (next.title.trim() && next.title !== manga.title) body.title = next.title.trim();
		if (!sameList(authors, manga.authors ?? [])) body.authors = authors;
		if (!sameList(artists, manga.artists ?? [])) body.artists = artists;
		if (next.description !== (manga.description ?? '')) body.description = next.description;
		if (!sameList(tags, manga.tags ?? [])) body.tags = tags;
		if (next.status !== manga.status) body.status = next.status;
		if (next.contentRating !== manga.content_rating) body.contentRating = next.contentRating;
		return body;
	};

	function update(field: keyof FormState, value: string) {
		setForm((cur) => (cur ? { ...cur, [field]: value } : cur));
	}

	function unlock(field: MetadataField) {
		patch.mutate({ id: mangaId, metadataUnlock: [field] });
	}

	async function restore() {
		if (!window.confirm(t('metadata.restoreConfirm'))) return;
		setRestoring(true);
		try {
			const result = await restoreMetadata(mangaId, { refresh: true });
			setForm(formFrom(result.manga));
			onClose();
		} finally {
			setRestoring(false);
		}
	}

	function submit(event: FormEvent) {
		event.preventDefault();
		if (!form || !manga) return;
		const body = patchBody(form);
		if (Object.keys(body).length <= 1) {
			onClose();
			return;
		}
		patch.mutate(body, { onSuccess: () => onClose() });
	}

	return (
		<Modal
			open
			title={t('metadata.editTitle')}
			onClose={onClose}
			panelClassName="modal__panel--wide"
			footer={
				<>
					<Button onClick={() => void restore()} loading={restoring}>
						<RotateCcw size={14} /> {t('metadata.restoreSource')}
					</Button>
					<Button onClick={onClose} disabled={patch.isPending}>
						{t('common.cancel')}
					</Button>
					<Button
						variant="primary"
						loading={patch.isPending}
						disabled={!form}
						onClick={() => formRef.current?.requestSubmit()}
					>
						{t('common.save')}
					</Button>
				</>
			}
		>
			{mangaQuery.isPending || !form ? (
				<div className="tracker-dialog-state">
					<Spinner />
					{t('common.loading')}
				</div>
			) : (
				<form ref={formRef} className="metadata-dialog metadata-edit" onSubmit={submit}>
					<p className="muted metadata-edit__hint">{t('metadata.editHint')}</p>
					{EDITABLE.map((field) => {
						const lock = lockOf(field);
						return (
							<label className="metadata-edit__row" key={field}>
								<span className="metadata-edit__label">
									{t(`metadata.fieldLabels.${field}`)}
									<LockNote
										lock={lock}
										sourceName={sourceName}
										fieldLabel={t(`metadata.fieldLabels.${field}`)}
										onUnlock={() => unlock(field)}
									/>
								</span>
								{field === 'description' ? (
									<textarea
										className="input metadata-edit__textarea"
										rows={4}
										value={form.description}
										onChange={(event) => update('description', event.currentTarget.value)}
									/>
								) : field === 'status' ? (
									<Select value={form.status} onChange={(value) => update('status', value)}>
										{STATUSES.map((status) => (
											<option key={status} value={status}>
												{t(`format.status.${status.toLowerCase()}`)}
											</option>
										))}
									</Select>
								) : field === 'contentRating' ? (
									<Select
										value={form.contentRating}
										onChange={(value) => update('contentRating', value)}
									>
										{RATINGS.map((rating) => (
											<option key={rating} value={rating}>
												{t(`metadata.contentRatings.${rating.toLowerCase()}`)}
											</option>
										))}
									</Select>
								) : (
									<Input
										value={form[field]}
										onChange={(event) => update(field, event.currentTarget.value)}
									/>
								)}
							</label>
						);
					})}
					{patch.isError && (
						<p className="tracker-error" role="alert">
							{patch.error.message}
						</p>
					)}
					{patch.isSuccess && (
						<p className="metadata-edit__saved" role="status">
							{t('metadata.editSaved')}
						</p>
					)}
				</form>
			)}
		</Modal>
	);
}

function LockNote({
	lock,
	sourceName,
	fieldLabel,
	onUnlock
}: {
	lock: { provider: string } | undefined;
	sourceName: (id: number) => string | undefined;
	fieldLabel: string;
	onUnlock: () => void;
}) {
	const { t } = useTranslation();
	if (!lock) {
		return <span className="metadata-edit__source">{t('metadata.sourceFollows')}</span>;
	}
	const label =
		lock.provider === 'manual'
			? t('metadata.manualLock')
			: lockProviderLabel(lock.provider, sourceName);
	return (
		<span className="metadata-edit__source metadata-edit__source--locked">
			{t('metadata.lockedBy', { provider: label || lock.provider })}
			<button
				type="button"
				className="metadata-edit__unlock"
				aria-label={`${t('metadata.unlock')} ${fieldLabel}`}
				onClick={(event) => {
					event.preventDefault();
					onUnlock();
				}}
			>
				<LockOpen size={12} />
			</button>
		</span>
	);
}
