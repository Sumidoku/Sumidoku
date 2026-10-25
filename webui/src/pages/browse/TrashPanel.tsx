import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { RotateCcw, Trash, Trash2 } from 'lucide-react';
import { ApiError, coverUrl } from '../../api/client';
import { useEmptyTrash, useLocalTrash, usePurgeTrash, useRestoreTrash, useSources } from '../../api/hooks';
import type { LocalTrashEntry } from '../../api/types';
import { translateApiError } from '../../i18n';
import { formatDate } from '../../lib/format';
import { Button, Cover, EmptyState, Modal, Skeleton, pushToast } from '../../components';

/** Browse page's trash tab: local manga whose directories vanished from the last scan */
export function TrashPanel() {
	const { t } = useTranslation();
	const trash = useLocalTrash();
	const sources = useSources();
	const restore = useRestoreTrash();
	const purge = usePurgeTrash();
	const emptyTrash = useEmptyTrash();
	const [pendingPurge, setPendingPurge] = useState<LocalTrashEntry | null>(null);
	const [confirmEmpty, setConfirmEmpty] = useState(false);

	const sourceName = (id: number) => sources.data?.find((s) => s.id === id)?.name ?? `#${id}`;

	async function onRestore(entry: LocalTrashEntry) {
		try {
			await restore.mutateAsync(entry.manga.id);
			pushToast(t('browse.trashRestored', { name: entry.manga.title }), 'info');
		} catch (e) {
			// the mutation is silent: a 409 'gone' (the directory is still missing) surfaces the conflict message here
			if (e instanceof ApiError) pushToast(translateApiError(e.code, e.message, { status: e.status }));
		}
	}

	async function onPurge() {
		if (!pendingPurge) return;
		try {
			await purge.mutateAsync(pendingPurge.manga.id);
			pushToast(t('browse.trashPurged', { name: pendingPurge.manga.title }), 'info');
			setPendingPurge(null);
		} catch {
			// client already popped an error toast
		}
	}

	async function onEmptyTrash() {
		try {
			await emptyTrash.mutateAsync(undefined);
			pushToast(t('browse.trashEmptyQueued'), 'info');
			setConfirmEmpty(false);
		} catch {
			// client already popped an error toast
		}
	}

	if (trash.isPending) {
		return (
			<div className="source-list">
				{Array.from({ length: 3 }, (_, i) => (
					<Skeleton key={i} height={72} style={{ borderRadius: 'var(--r-md)' }} />
				))}
			</div>
		);
	}
	const entries = trash.data ?? [];
	if (entries.length === 0) {
		return <EmptyState icon={Trash2} title={t('browse.trashEmptyTitle')} hint={t('browse.trashEmptyHint')} />;
	}

	return (
		<>
			<div className="trash__head">
				<span className="faint">{t('browse.trashCount', { count: entries.length })}</span>
				<Button size="sm" variant="danger" onClick={() => setConfirmEmpty(true)}>
					<Trash size={14} />
					{t('browse.emptyTrash')}
				</Button>
			</div>
			<div className="source-list">
				{entries.map((entry) => (
					<div key={entry.manga.id} className="trash-row">
						<div className="trash-row__cover">
							<Cover src={coverUrl(entry.manga.id)} title={entry.manga.title} />
						</div>
						<div className="trash-row__main">
							<div className="trash-row__title">{entry.manga.title}</div>
							<div className="trash-row__meta">
								{t('browse.trashMeta', {
									source: sourceName(entry.manga.sourceId),
									chapters: entry.trashedChapters,
									date: formatDate(entry.trashedAt)
								})}
							</div>
						</div>
						<div className="trash-row__actions">
							<Button
								size="sm"
								loading={restore.isPending && restore.variables === entry.manga.id}
								onClick={() => void onRestore(entry)}
							>
								<RotateCcw size={14} />
								{t('browse.trashRestore')}
							</Button>
							<Button size="sm" variant="danger" onClick={() => setPendingPurge(entry)}>
								<Trash2 size={14} />
								{t('browse.trashPurge')}
							</Button>
						</div>
					</div>
				))}
			</div>

			<Modal
				open={pendingPurge !== null}
				title={t('browse.trashPurgeTitle', { name: pendingPurge?.manga.title ?? '' })}
				onClose={() => setPendingPurge(null)}
				footer={
					<>
						<Button onClick={() => setPendingPurge(null)}>{t('common.cancel')}</Button>
						<Button variant="danger" loading={purge.isPending} onClick={() => void onPurge()}>
							{t('browse.trashPurge')}
						</Button>
					</>
				}
			>
				<p className="muted">{t('browse.trashPurgeHint')}</p>
			</Modal>
			<Modal
				open={confirmEmpty}
				title={t('browse.emptyTrashTitle')}
				onClose={() => setConfirmEmpty(false)}
				footer={
					<>
						<Button onClick={() => setConfirmEmpty(false)}>{t('common.cancel')}</Button>
						<Button variant="danger" loading={emptyTrash.isPending} onClick={() => void onEmptyTrash()}>
							{t('browse.emptyTrashConfirm')}
						</Button>
					</>
				}
			>
				<p className="muted">{t('browse.emptyTrashHint')}</p>
			</Modal>
		</>
	);
}
