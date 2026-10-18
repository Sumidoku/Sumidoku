import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { ArrowRight, Search } from 'lucide-react';
import { coverUrl } from '../../api/client';
import { migrateLookup, useMigrate, useRegisterManga, useSources } from '../../api/hooks';
import type { Manga, MangaDetail, MigrateMode } from '../../api/types';
import { mapPool } from '../../lib/pool';
import { Button, Cover, Modal, Spinner, pushToast } from '../../components';
import { DestinationPicker } from '../migrate/DestinationPicker';
import '../migrate/migrate.css';

const MODE_LABEL: Record<MigrateMode, string> = {
	copy: '复制',
	migrate: '迁移',
	migrateWithHistory: '迁移（保留历史）'
};

const MODE_HINT: Record<MigrateMode, string> = {
	copy: '目标入库并搬走进度，当前条目原样保留',
	migrate: '目标入库并搬走进度，当前条目撤出书库并清掉阅读进度',
	migrateWithHistory: '目标入库并搬走进度，当前条目撤出书库但保留阅读历史'
};

/**
 * 单条目切换图源：选目标组合 → 对全部目标源 3 并发标题搜索 → 用户挑匹配 →
 * 选模式执行（或仅查看条目）→ 跳目标页
 */
export function MigrateModal({ manga, onClose }: { manga: MangaDetail; onClose: () => void }) {
	const navigate = useNavigate();
	const [destinations, setDestinations] = useState<number[]>([]);
	const [searching, setSearching] = useState(false);
	const [results, setResults] = useState<Manga[] | null>(null);
	const [picked, setPicked] = useState<Manga | null>(null);
	const [error, setError] = useState<string | null>(null);
	const migrate = useMigrate();
	const register = useRegisterManga();
	const sources = useSources();
	const sourceName = (id: number) =>
		(sources.data ?? []).find((s) => s.id === id)?.name ?? '';

	async function search() {
		setSearching(true);
		setResults(null);
		setPicked(null);
		setError(null);
		const hits = await mapPool(destinations, 3, async (destId) => {
			try {
				return await migrateLookup(manga.id, destId);
			} catch (e) {
				setError(e instanceof Error ? e.message : '搜索失败');
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
						setError(item?.message || item?.reason || '迁移失败');
						return;
					}
					for (const w of item.warnings ?? []) pushToast(w, 'info');
					pushToast(`${MODE_LABEL[mode]}完成`, 'info');
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
		<Modal open title={`切换图源：《${manga.title}》`} onClose={onClose} panelClassName="migrate-single">
			{picked ? (
				<div className="migrate-single__confirm">
					<div className="migrate-single__pair">
						<span className="migrate-row__cover">
							<Cover src={coverUrl(manga.id)} title={manga.title} />
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
							仅查看条目
						</Button>
						<Button size="sm" disabled={busy} onClick={() => setPicked(null)}>
							返回候选
						</Button>
					</div>
					<p className="muted">{MODE_HINT.migrate}</p>
				</div>
			) : (
				<>
					<DestinationPicker selected={destinations} onChange={setDestinations} excludeId={manga.sourceId} />
					<div className="migrate-single__bar">
						<Button
							size="sm"
							variant="primary"
							loading={searching}
							disabled={destinations.length === 0}
							onClick={() => void search()}
						>
							<Search size={14} />
							搜索候选
						</Button>
						{error && <span className="migrate-single__error">{error}</span>}
					</div>
					{searching ? (
						<Spinner />
					) : results === null ? (
						<p className="muted">会对全部目标源按标题并发搜索（3 并发）。</p>
					) : results.length === 0 ? (
						<p className="muted">没有命中，可以调整目标组合再搜。</p>
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
