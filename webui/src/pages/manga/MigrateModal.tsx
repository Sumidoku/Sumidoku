import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { ArrowRight, Search } from 'lucide-react';
import { coverUrl } from '../../api/client';
import { migrateLookup, useMigrate, useRegisterManga, useSources } from '../../api/hooks';
import type { Manga, MigrateMode, SearchManga } from '../../api/types';
import { mapPool } from '../../lib/pool';
import { Button, Cover, Modal, Spinner, pushToast } from '../../components';
import { DestinationPicker } from '../migrate/DestinationPicker';
import '../migrate/migrate.css';

/**
 * Switch a single entry to another source: pick the destination set → title-search all target sources
 * with 3-way concurrency → user picks the match → execute with the chosen mode (or just view the entry) → jump to the target page
 */
export function MigrateModal({ manga, onClose }: { manga: Manga; onClose: () => void }) {
	const { t } = useTranslation();
	const navigate = useNavigate();
	const [destinations, setDestinations] = useState<number[]>([]);
	const [searching, setSearching] = useState(false);
	const [results, setResults] = useState<SearchManga[] | null>(null);
	const [picked, setPicked] = useState<SearchManga | null>(null);
	const [error, setError] = useState<string | null>(null);
	const migrate = useMigrate();
	const register = useRegisterManga();
	const sources = useSources();
	const sourceName = (id: number) =>
		(sources.data ?? []).find((s) => s.id === id)?.name ?? '';

	const MODE_LABEL: Record<MigrateMode, string> = {
		copy: t('migrateModal.mode.copy'),
		migrate: t('migrateModal.mode.migrate'),
		migrateWithHistory: t('migrateModal.mode.migrateWithHistory')
	};
	const MODE_HINT: Record<MigrateMode, string> = {
		copy: t('migrateModal.modeHint.copy'),
		migrate: t('migrateModal.modeHint.migrate'),
		migrateWithHistory: t('migrateModal.modeHint.migrateWithHistory')
	};

	async function search() {
		setSearching(true);
		setResults(null);
		setPicked(null);
		setError(null);
		const hits = await mapPool(destinations, 3, async (destId) => {
			try {
				return await migrateLookup(manga.id, destId);
			} catch (e) {
				setError(e instanceof Error ? e.message : t('migrateModal.searchFailed'));
				return null;
			}
		});
		setResults(hits.flatMap((h) => h?.candidates ?? []));
		setSearching(false);
	}

	function execute(mode: MigrateMode) {
		if (!picked) return;
		migrate.mutate(
			{
				mode,
				entries: [
					{
						fromMangaId: manga.id,
						toSourceId: picked.sourceId!,
						toMangaKey: picked.key,
						toManga: picked
					}
				]
			},
			{
				onSuccess: (res) => {
					const item = res.results[0];
					if (!item?.ok) {
						setError(item?.message || item?.reason || t('migrateModal.migrateFailed'));
						return;
					}
					for (const w of item.warnings ?? []) pushToast(w, 'info');
					pushToast(t('migrateModal.done', { mode: MODE_LABEL[mode] }), 'info');
					onClose();
					navigate(`/manga/${item.toMangaId}`);
				}
			}
		);
	}

	function showEntry() {
		if (!picked) return;
		register.mutate(
			{ sourceId: picked.sourceId!, manga: picked },
			{
				onSuccess: (res) => {
					onClose();
					navigate(`/manga/${res.id}`);
				},
				onError: (e) => setError(e.message)
			}
		);
	}

	const busy = migrate.isPending || register.isPending;

	return (
		<Modal open title={t('migrateModal.title', { title: manga.title })} onClose={onClose} panelClassName="migrate-single">
			{picked ? (
				<div className="migrate-single__confirm">
					<div className="migrate-single__pair">
						<span className="migrate-row__cover">
							<Cover src={coverUrl(manga.id, manga.cover)} title={manga.title} />
						</span>
						<ArrowRight size={16} />
						<div>
							<div className="migrate-row__title">{picked.title}</div>
							<div className="migrate-row__meta">{sourceName(picked.sourceId!)}</div>
						</div>
					</div>
					{error && <p className="migrate-single__error">{error}</p>}
					<div className="migrate-single__actions">
						{(['copy', 'migrate', 'migrateWithHistory'] as MigrateMode[]).map((mode) => (
							<Button key={mode} size="sm" disabled={busy} title={MODE_HINT[mode]} onClick={() => execute(mode)}>
								{MODE_LABEL[mode]}
							</Button>
						))}
						<Button size="sm" disabled={busy} onClick={showEntry}>
							{t('migrateModal.viewOnly')}
						</Button>
						<Button size="sm" disabled={busy} onClick={() => setPicked(null)}>
							{t('migrateModal.backToCandidates')}
						</Button>
					</div>
					<p className="muted">{MODE_HINT.migrate}</p>
				</div>
			) : (
				<>
					<DestinationPicker selected={destinations} onChange={setDestinations} />
					<div className="migrate-single__bar">
						<Button
							size="sm"
							variant="primary"
							loading={searching}
							disabled={destinations.length === 0}
							onClick={() => void search()}
						>
							<Search size={14} />
							{t('migrateModal.searchCandidates')}
						</Button>
						{error && <span className="migrate-single__error">{error}</span>}
					</div>
					{searching ? (
						<Spinner />
					) : results === null ? (
						<p className="muted">{t('migrateModal.searchHint')}</p>
					) : results.length === 0 ? (
						<p className="muted">{t('migrateModal.noHits')}</p>
					) : (
						<div className="migrate-rematch__list">
							{results.map((c) => (
								<button
									key={`${c.sourceId}:${c.key}`}
									type="button"
									className="migrate-rematch__item"
									onClick={() => setPicked(c)}
								>
									<span className="migrate-rematch__title">{c.title}</span>
									<span className="migrate-rematch__source">{sourceName(c.sourceId!)}</span>
								</button>
							))}
						</div>
					)}
				</>
			)}
		</Modal>
	);
}
