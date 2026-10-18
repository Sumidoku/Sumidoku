import { useMemo, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { ArrowLeft, ArrowRight, Ellipsis, Search, TriangleAlert } from 'lucide-react';
import { coverUrl } from '../../api/client';
import { migrateLookup, useLibrary, useMigrate, useMigrateSources } from '../../api/hooks';
import type { LibraryEntry, Manga, MigrateMode, MigrateResultItem } from '../../api/types';
import { mapPool } from '../../lib/pool';
import {
	Button,
	Cover,
	EmptyState,
	Input,
	Modal,
	PageHeader,
	Skeleton,
	Spinner,
	pushToast
} from '../../components';
import { DestinationPicker } from './DestinationPicker';
import './migrate.css';

type Step = 'source' | 'series' | 'destinations' | 'match';

interface Row {
	manga: LibraryEntry;
	state: 'pending' | 'matching' | 'matched' | 'notfound';
	target?: Manga;
	result?: MigrateResultItem;
}

const MODE_LABEL: Record<MigrateMode, string> = {
	copy: '复制',
	migrate: '迁移',
	migrateWithHistory: '迁移（保留历史）'
};

const MODE_HINT: Record<MigrateMode, string> = {
	copy: '目标入库并搬走进度，源条目原样保留',
	migrate: '目标入库并搬走进度，源条目撤出书库并清掉阅读进度',
	migrateWithHistory: '目标入库并搬走进度，源条目撤出书库但保留阅读历史'
};

/** 批量自动匹配：按目标优先级逐源标题搜索，首个有结果的源胜出并取其第一条 */
async function autoMatch(
	rows: Row[],
	destinations: number[],
	update: (id: number, patch: Partial<Row>) => void
) {
	await mapPool(rows, 3, async (row) => {
		update(row.manga.id, { state: 'matching' });
		for (const destId of destinations) {
			try {
				const r = await migrateLookup(row.manga.id, destId);
				if (r.candidates.length > 0) {
					update(row.manga.id, { state: 'matched', target: r.candidates[0] });
					return;
				}
			} catch {
				// 单个目标源搜索失败按没命中处理，继续下一个目标源
			}
		}
		update(row.manga.id, { state: 'notfound' });
	});
}

function Reason({ item }: { item: MigrateResultItem }) {
	const text: Record<string, string> = {
		not_found: '源条目不存在',
		not_in_library: '源条目不在书库',
		source_not_found: '目标源不存在',
		same_source: '目标与来源是同一条目',
		fetch_failed: item.message ? `目标详情拉取失败：${item.message}` : '目标详情拉取失败'
	};
	return <span className="migrate-row__reason">{text[item.reason ?? ''] ?? item.reason}</span>;
}

export default function MigratePage() {
	const [params] = useSearchParams();
	// breaking-change 引导入口：?source=<id> 预填来源，sameSource=1 允许并预选同源目标
	const prefill = Number(params.get('source')) || null;
	const sameSource = params.get('sameSource') === '1';
	const [step, setStep] = useState<Step>(prefill ? 'series' : 'source');
	const [sourceId, setSourceId] = useState<number | null>(prefill);
	const [selected, setSelected] = useState<Set<number>>(new Set());
	const [destinations, setDestinations] = useState<number[]>(
		sameSource && prefill ? [prefill] : []
	);
	const [rows, setRows] = useState<Row[]>([]);
	const [matching, setMatching] = useState(false);
	const [confirmMode, setConfirmMode] = useState<MigrateMode | null>(null);
	const [rematchRow, setRematchRow] = useState<Row | null>(null);
	const [executed, setExecuted] = useState(false);

	const sourcesQuery = useMigrateSources();
	const libraryQuery = useLibrary(undefined, { sourceId: sourceId ?? 0 });
	const migrate = useMigrate();

	const series = useMemo(
		() => (sourceId === null ? [] : (libraryQuery.data?.pages.flatMap((p) => p.entries) ?? [])),
		[libraryQuery.data, sourceId]
	);
	const sourceName = (sourcesQuery.data ?? []).find((s) => s.id === sourceId)?.name ?? '';
	const destName = (id: number) => (sourcesQuery.data ?? []).find((s) => s.id === id)?.name ?? `#${id}`;

	function patchRow(id: number, patch: Partial<Row>) {
		setRows((prev) => prev.map((r) => (r.manga.id === id ? { ...r, ...patch } : r)));
	}

	function pickSource(id: number) {
		setSourceId(id);
		setSelected(new Set());
		setStep('series');
	}

	function startMatch() {
		const picked = series.filter((m) => selected.has(m.id));
		setRows(picked.map((manga) => ({ manga, state: 'pending' })));
		setExecuted(false);
		setStep('match');
		setMatching(true);
		const initial: Row[] = picked.map((manga) => ({ manga, state: 'pending' }));
		void autoMatch(initial, destinations, patchRow).finally(() => setMatching(false));
	}

	const matchedRows = rows.filter((r) => r.state === 'matched' && r.target);
	const doneCount = rows.filter((r) => r.result?.ok).length;

	function execute(mode: MigrateMode) {
		const entries = matchedRows
			.filter((r) => !r.result?.ok)
			.map((r) => ({
				fromMangaId: r.manga.id,
				toSourceId: r.target!.sourceId!,
				toMangaKey: r.target!.key,
				toManga: r.target
			}));
		if (entries.length === 0) return;
		migrate.mutate(
			{ mode, entries },
			{
				onSuccess: (res) => {
					setExecuted(true);
					setConfirmMode(null);
					for (const item of res.results) {
						patchRow(item.fromMangaId, { result: item });
					}
					const failed = res.results.filter((r) => !r.ok).length;
					pushToast(
						failed === 0
							? `已迁移 ${res.results.length} 部`
							: `完成 ${res.results.length - failed} 部，失败 ${failed} 部`,
						'info'
					);
					for (const item of res.results) {
						for (const w of item.warnings ?? []) pushToast(w, 'info');
					}
				}
			}
		);
	}

	return (
		<div className="page container">
			<PageHeader
				title="迁移"
				actions={
					step !== 'source' ? (
						<Button
							size="sm"
							onClick={() => setStep(step === 'series' ? 'source' : step === 'destinations' ? 'series' : 'destinations')}
						>
							<ArrowLeft size={14} />
							上一步
						</Button>
					) : undefined
				}
			/>

			{step === 'source' &&
				(sourcesQuery.isPending ? (
					<div className="migrate-source-list">
						{Array.from({ length: 3 }, (_, i) => (
							<Skeleton key={i} height={64} style={{ borderRadius: 'var(--r-md)' }} />
						))}
					</div>
				) : (
					<>
						<p className="muted">先选要迁出的源（只列在库有条目的源）：</p>
						<div className="migrate-source-list">
							{(sourcesQuery.data ?? [])
								.filter((s) => s.mangaCount > 0)
								.map((s) => (
									<button
										key={s.id}
										type="button"
										className="migrate-source"
										onClick={() => pickSource(s.id)}
									>
										<span className="migrate-source__name">{s.name}</span>
										<span className="migrate-source__count">在库 {s.mangaCount} 部</span>
										<ArrowRight size={16} />
									</button>
								))}
						</div>
						{(sourcesQuery.data ?? []).every((s) => s.mangaCount === 0) && (
							<EmptyState icon={TriangleAlert} title="书库还是空的" hint="先把漫画加入书库再来迁移" />
						)}
					</>
				))}

			{step === 'series' && (
				<>
					<div className="migrate-toolbar">
						<span>
							来源：<b>{sourceName}</b> · 已选 {selected.size} / {series.length} 部
						</span>
						<Button
							size="sm"
							onClick={() =>
								setSelected(selected.size === series.length ? new Set() : new Set(series.map((m) => m.id)))
							}
						>
							{selected.size === series.length ? '清空' : '全选'}
						</Button>
						<Button size="sm" variant="primary" disabled={selected.size === 0} onClick={() => setStep('destinations')}>
							选目标源
							<ArrowRight size={14} />
						</Button>
					</div>
					<div className="migrate-series">
						{series.map((m) => (
							<label key={m.id} className="migrate-series__item">
								<input
									type="checkbox"
									checked={selected.has(m.id)}
									onChange={() =>
										setSelected((prev) => {
											const next = new Set(prev);
											if (next.has(m.id)) next.delete(m.id);
											else next.add(m.id);
											return next;
										})
									}
								/>
								<span className="migrate-series__cover">
									<Cover src={coverUrl(m.id)} title={m.title} />
								</span>
								<span className="migrate-series__title">{m.title}</span>
							</label>
						))}
					</div>
					{libraryQuery.hasNextPage && (
						<Button size="sm" loading={libraryQuery.isFetchingNextPage} onClick={() => void libraryQuery.fetchNextPage()}>
							加载更多
						</Button>
					)}
				</>
			)}

			{step === 'destinations' && (
				<>
					<DestinationPicker
						selected={destinations}
						onChange={setDestinations}
						excludeId={sameSource ? undefined : (sourceId ?? undefined)}
					/>
					<div className="migrate-toolbar">
						<Button variant="primary" disabled={destinations.length === 0} onClick={startMatch}>
							开始匹配（{selected.size} 部）
							<ArrowRight size={14} />
						</Button>
					</div>
				</>
			)}

			{step === 'match' && (
				<>
					<div className="migrate-toolbar">
						<span>
							目标：{destinations.map(destName).join(' → ')} · 命中 {matchedRows.length} / {rows.length}
							{matching && ' · 匹配中…'}
							{executed && ` · 已迁移 ${doneCount}`}
						</span>
						{matching && <Spinner size={16} />}
						{!matching && !executed && (
							<>
								<Button size="sm" onClick={() => void autoMatch(rows.filter((r) => r.state === 'notfound'), destinations, patchRow)}>
									重试未命中
								</Button>
								{(['copy', 'migrate', 'migrateWithHistory'] as MigrateMode[]).map((mode) => (
									<Button
										key={mode}
										size="sm"
										variant={mode === 'migrate' ? 'primary' : undefined}
										disabled={matchedRows.length === 0}
										onClick={() => setConfirmMode(mode)}
									>
										{MODE_LABEL[mode]}
									</Button>
								))}
							</>
						)}
					</div>
					<div className="migrate-rows">
						{rows.map((row) => (
							<div key={row.manga.id} className="migrate-row">
								<span className="migrate-row__cover">
									<Cover src={coverUrl(row.manga.id)} title={row.manga.title} />
								</span>
								<div className="migrate-row__side">
									<div className="migrate-row__title">{row.manga.title}</div>
									<div className="migrate-row__meta">{sourceName}</div>
								</div>
								<ArrowRight size={16} className="migrate-row__arrow" />
								<div className="migrate-row__side">
									{row.state === 'matching' || (matching && row.state === 'pending') ? (
										<Spinner size={16} />
									) : row.target ? (
										<>
											<div className="migrate-row__title">{row.target.title}</div>
											<div className="migrate-row__meta">{destName(row.target.sourceId!)}</div>
										</>
									) : (
										<div className="migrate-row__notfound">未找到</div>
									)}
									{row.result &&
										(row.result.ok ? (
											<Link className="migrate-row__goto" to={`/manga/${row.result.toMangaId}`}>
												已迁移，查看目标 →
											</Link>
										) : (
											<Reason item={row.result} />
										))}
								</div>
								{!executed && !matching && (
									<RowMenu
										onRematch={() => setRematchRow(row)}
										onRemove={() => setRows((prev) => prev.filter((r) => r.manga.id !== row.manga.id))}
									/>
								)}
							</div>
						))}
					</div>
				</>
			)}

			<Modal
				open={confirmMode !== null}
				title={confirmMode ? `${MODE_LABEL[confirmMode]} ${matchedRows.length} 部？` : ''}
				onClose={() => setConfirmMode(null)}
				footer={
					<>
						<Button onClick={() => setConfirmMode(null)}>取消</Button>
						<Button
							variant="primary"
							loading={migrate.isPending}
							onClick={() => confirmMode && execute(confirmMode)}
						>
							确认{confirmMode ? MODE_LABEL[confirmMode] : ''}
						</Button>
					</>
				}
			>
				<p className="muted">{confirmMode ? MODE_HINT[confirmMode] : ''}</p>
				<p className="muted">未命中的 {rows.length - matchedRows.length} 部不会动。</p>
			</Modal>

			{rematchRow && (
				<RematchModal
					row={rematchRow}
					destinations={destinations}
					destName={destName}
					onPick={(candidate) => {
						patchRow(rematchRow.manga.id, { state: 'matched', target: candidate });
						setRematchRow(null);
					}}
					onClose={() => setRematchRow(null)}
				/>
			)}
		</div>
	);
}

function RowMenu({ onRematch, onRemove }: { onRematch: () => void; onRemove: () => void }) {
	const [open, setOpen] = useState(false);
	return (
		<span className="migrate-row__menu">
			<button type="button" className="migrate-row__menu-btn" title="更多操作" onClick={() => setOpen((v) => !v)}>
				<Ellipsis size={16} />
			</button>
			{open && (
				<span className="migrate-row__menu-pop card">
					<button
						type="button"
						onClick={() => {
							setOpen(false);
							onRematch();
						}}
					>
						重新搜索
					</button>
					<button
						type="button"
						onClick={() => {
							setOpen(false);
							onRemove();
						}}
					>
						不迁移这部
					</button>
				</span>
			)}
		</span>
	);
}

/** 单行手动重搜：对全部目标源 3 并发标题搜索，用户挑候选 */
function RematchModal({
	row,
	destinations,
	destName,
	onPick,
	onClose
}: {
	row: Row;
	destinations: number[];
	destName: (id: number) => string;
	onPick: (candidate: Manga) => void;
	onClose: () => void;
}) {
	const [query, setQuery] = useState(row.manga.title);
	const [searching, setSearching] = useState(false);
	const [results, setResults] = useState<Manga[] | null>(null);

	async function search() {
		setSearching(true);
		setResults(null);
		const hits = await mapPool(destinations, 3, async (destId) => {
			try {
				return await migrateLookup(row.manga.id, destId, query);
			} catch {
				return null;
			}
		});
		setResults(hits.flatMap((h) => h?.candidates ?? []));
		setSearching(false);
	}

	return (
		<Modal open title={`重新搜索《${row.manga.title}》`} onClose={onClose} panelClassName="migrate-rematch">
			<div className="migrate-rematch__bar">
				<Input
					icon={<Search size={15} />}
					value={query}
					onChange={(e) => setQuery(e.currentTarget.value)}
					onKeyDown={(e) => e.key === 'Enter' && void search()}
				/>
				<Button size="sm" variant="primary" loading={searching} disabled={!query.trim()} onClick={() => void search()}>
					搜索
				</Button>
			</div>
			{searching ? (
				<Spinner />
			) : results === null ? (
				<p className="muted">对全部目标源搜索，点一个候选作为匹配结果。</p>
			) : results.length === 0 ? (
				<p className="muted">没有命中，换个关键词试试。</p>
			) : (
				<div className="migrate-rematch__list">
					{results.map((c) => (
						<button key={`${c.sourceId}:${c.key}`} type="button" className="migrate-rematch__item" onClick={() => onPick(c)}>
							<span className="migrate-rematch__title">{c.title}</span>
							<span className="migrate-rematch__source">{destName(c.sourceId!)}</span>
						</button>
					))}
				</div>
			)}
		</Modal>
	);
}
