import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
	ArrowLeftRight,
	CircleArrowUp,
	GripVertical,
	House,
	PackageOpen,
	Pin,
	PinOff,
	ScanSearch,
	Trash
} from 'lucide-react';
import { sourceIconUrl } from '../api/client';
import {
	useDeleteSource,
	usePinSource,
	useReorderPinned,
	useScanSources,
	useSources,
	useUpdateAllSources,
	useUpdateSource
} from '../api/hooks';
import type { Source } from '../api/types';
import { contentRatingColor, contentRatingLabel, languageLabel } from '../lib/source';
import {
	Button,
	Chip,
	EmptyState,
	IconButton,
	Modal,
	PageHeader,
	SegmentedControl,
	Skeleton,
	pushToast
} from '../components';
import ReposPanel from './browse/ReposPanel';
import { SourceSettingsButton } from './browse/SourceSettingsModal';
import { iconColor } from './browse/iconColor';

function SourceIcon({ source }: { source: Source }) {
	const [failed, setFailed] = useState(false);
	if (failed) {
		return (
			<span className="source-card__icon-fallback" style={{ background: iconColor(source.key) }}>
				{source.name.slice(0, 1)}
			</span>
		);
	}
	return (
		<span className="source-card__icon">
			<img
				src={sourceIconUrl(source.id)}
				alt=""
				loading="lazy"
				referrerPolicy="no-referrer"
				onError={() => setFailed(true)}
			/>
		</span>
	);
}

function UpdateButton({ source }: { source: Source }) {
	const update = useUpdateSource();
	const navigate = useNavigate();

	async function onClick(e: React.MouseEvent) {
		e.stopPropagation();
		try {
			const r = await update.mutateAsync(source.id);
			pushToast(`已更新 ${source.name} 到 v${r.version}`, 'info');
			for (const w of r.migrationWarnings.slice(0, 3)) pushToast(w, 'info');
			if (r.migrationRequired) {
				pushToast(`${source.name} 换了条目地址，库内条目需要迁移`, 'info');
				navigate(`/migrate?source=${source.id}&sameSource=1`);
			}
		} catch {
			// client 已弹错误 toast
		}
	}

	return (
		<Button size="sm" variant="primary" loading={update.isPending} onClick={(e) => void onClick(e)}>
			<CircleArrowUp size={14} />
			v{source.updateVersion}
		</Button>
	);
}

/** 置顶段卡片开启拖拽排序时传入；普通列表不传 */
interface CardDnd {
	dragging: boolean;
	dragOver: boolean;
	onDragStart: (e: React.DragEvent) => void;
	onDragOver: (e: React.DragEvent) => void;
	onDrop: () => void;
	onDragEnd: () => void;
}

function SourceCard({
	source: s,
	dnd,
	onTogglePin,
	onDelete
}: {
	source: Source;
	dnd?: CardDnd;
	onTogglePin: (s: Source) => void;
	onDelete: (s: Source) => void;
}) {
	const navigate = useNavigate();
	const cls = ['source-card'];
	if (s.enabled) cls.push('source-card--link');
	else cls.push('source-card--disabled');
	if (dnd?.dragging) cls.push('source-card--dragging');
	if (dnd?.dragOver) cls.push('source-card--drag-over');
	return (
		<div
			className={cls.join(' ')}
			role={s.enabled ? 'link' : undefined}
			tabIndex={s.enabled ? 0 : undefined}
			onClick={() => s.enabled && navigate(`/browse/${s.id}`)}
			onKeyDown={(e) => {
				if (e.target !== e.currentTarget) return;
				if (s.enabled && (e.key === 'Enter' || e.key === ' ')) {
					e.preventDefault();
					navigate(`/browse/${s.id}`);
				}
			}}
			onDragOver={dnd?.onDragOver}
			onDrop={dnd?.onDrop}
		>
			{dnd && (
				// 只让手柄可拖：整卡 draggable 会把点按导航也变成拖拽起手
				<span
					className="source-card__grip"
					draggable
					onDragStart={dnd.onDragStart}
					onDragEnd={dnd.onDragEnd}
				>
					<GripVertical size={16} />
				</span>
			)}
			<SourceIcon source={s} />
			<div className="source-card__main">
				<div className="source-card__name">
					<span className="source-card__name-text">{s.name}</span>
					{s.contentRating > 0 && (
						<Chip color={contentRatingColor(s.contentRating)}>
							{contentRatingLabel(s.contentRating)}
						</Chip>
					)}
				</div>
				<div className="source-card__meta">
					{s.key} · v{s.version}
					{!s.enabled ? ' · 已停用' : ''}
				</div>
				{s.languages.length > 0 && (
					<div className="source-card__badges">
						{s.languages.slice(0, 3).map((lang) => (
							<Chip key={lang}>{languageLabel(lang)}</Chip>
						))}
						{s.languages.length > 3 && <Chip>+{s.languages.length - 3}</Chip>}
					</div>
				)}
			</div>
			{s.updateVersion !== null && <UpdateButton source={s} />}
			<IconButton
				label={s.pinned ? `取消置顶 ${s.name}` : `置顶 ${s.name}`}
				onClick={(e) => {
					e.stopPropagation();
					onTogglePin(s);
				}}
			>
				{s.pinned ? <PinOff size={17} /> : <Pin size={17} />}
			</IconButton>
			<IconButton
				label={`${s.name} 主页`}
				onClick={(e) => {
					e.stopPropagation();
					navigate(`/browse/${s.id}/home`);
				}}
			>
				<House size={17} />
			</IconButton>
			<SourceSettingsButton sourceId={s.id} sourceName={s.name} />
			<IconButton
				label={`卸载 ${s.name}`}
				variant="danger"
				onClick={(e) => {
					e.stopPropagation();
					onDelete(s);
				}}
			>
				<Trash size={17} />
			</IconButton>
		</div>
	);
}

export default function BrowsePage() {
	const sources = useSources();
	const scan = useScanSources();
	const del = useDeleteSource();
	const updateAll = useUpdateAllSources();
	const pin = usePinSource();
	const reorderPinned = useReorderPinned();
	const navigate = useNavigate();
	const [pendingDelete, setPendingDelete] = useState<Source | null>(null);
	const [tab, setTab] = useState<'installed' | 'repos'>('installed');
	const [dragIndex, setDragIndex] = useState<number | null>(null);
	const [overIndex, setOverIndex] = useState<number | null>(null);

	const all = sources.data ?? [];
	const updateCount = all.filter((s) => s.updateVersion !== null).length;
	const pinnedSources = all
		.filter((s) => s.pinned)
		.sort((a, b) => (a.pinOrder ?? 0) - (b.pinOrder ?? 0));

	async function onScan() {
		try {
			const r = await scan.mutateAsync();
			pushToast(
				`扫描完成：安装 ${r.installed.length}、更新 ${r.updated.length}、跳过 ${r.skipped.length}`,
				'info'
			);
			for (const s of r.skipped.slice(0, 3)) {
				pushToast(`跳过 ${s.dir}：${s.reason}`, 'info');
			}
		} catch {
			// client 已弹错误 toast
		}
	}

	async function onUpdateAll() {
		try {
			const r = await updateAll.mutateAsync();
			pushToast(`更新完成：${r.updated.length} 个已更新、${r.failed.length} 个失败`, 'info');
			for (const f of r.failed.slice(0, 3)) {
				pushToast(`更新 ${f.key} 失败：${f.reason}`, 'info');
			}
			for (const w of r.migrationWarnings.slice(0, 3)) {
				pushToast(`${w.key}：${w.message}`, 'info');
			}
			if (r.migrationRequired.length > 0) {
				pushToast(`${r.migrationRequired.join('、')} 换了条目地址，库内条目需要迁移`, 'info');
				navigate('/migrate');
			}
		} catch {
			// client 已弹错误 toast
		}
	}

	async function togglePin(s: Source) {
		try {
			await pin.mutateAsync({ id: s.id, pinned: !s.pinned });
			pushToast(s.pinned ? `已取消置顶 ${s.name}` : `已置顶 ${s.name}`, 'info');
		} catch {
			// client 已弹错误 toast
		}
	}

	function onDropPinned(target: number) {
		const from = dragIndex;
		setDragIndex(null);
		setOverIndex(null);
		if (from === null || from === target) return;
		const keys = pinnedSources.map((s) => s.key);
		const [moved] = keys.splice(from, 1);
		keys.splice(target, 0, moved);
		reorderPinned.mutate(keys);
	}

	async function confirmDelete() {
		if (!pendingDelete) return;
		try {
			await del.mutateAsync(pendingDelete.id);
			pushToast(`已卸载 ${pendingDelete.name}`, 'info');
			setPendingDelete(null);
		} catch {
			// client 已弹错误 toast
		}
	}

	return (
		<div className="page container">
			<PageHeader
				title="浏览"
				actions={
					tab === 'installed' ? (
						<>
							<Button onClick={() => navigate('/migrate')}>
								<ArrowLeftRight size={15} />
								迁移
							</Button>
							{updateCount > 0 && (
								<Button loading={updateAll.isPending} onClick={() => void onUpdateAll()}>
									<CircleArrowUp size={15} />
									全部更新（{updateCount}）
								</Button>
							)}
							<Button variant="primary" loading={scan.isPending} onClick={() => void onScan()}>
								<ScanSearch size={15} />
								扫描源目录
							</Button>
						</>
					) : undefined
				}
			/>

			<div className="browse__tabs">
				<SegmentedControl
					options={[
						{ value: 'installed', label: '已安装' },
						{ value: 'repos', label: '源仓库' }
					]}
					value={tab}
					onChange={setTab}
				/>
			</div>

			{tab === 'repos' ? (
				<ReposPanel />
			) : sources.isPending ? (
				<div className="source-list">
					{Array.from({ length: 4 }, (_, i) => (
						<Skeleton key={i} height={82} style={{ borderRadius: 'var(--r-md)' }} />
					))}
				</div>
			) : all.length === 0 ? (
				<EmptyState
					icon={PackageOpen}
					title="还没有安装源"
					hint="把源包放进服务器的 sources 目录然后点「扫描源目录」，或到「源仓库」一键安装"
				/>
			) : (
				<>
					{pinnedSources.length > 0 && (
						<>
							<h2 className="browse__section">置顶</h2>
							<div className="source-list">
								{pinnedSources.map((s, i) => (
									<SourceCard
										key={s.id}
										source={s}
										dnd={{
											dragging: dragIndex === i,
											dragOver: overIndex === i && dragIndex !== null && dragIndex !== i,
											onDragStart: (e) => {
												e.dataTransfer.setData('text/plain', s.key);
												e.dataTransfer.effectAllowed = 'move';
												setDragIndex(i);
											},
											onDragOver: (e) => {
												e.preventDefault();
												setOverIndex(i);
											},
											onDrop: () => onDropPinned(i),
											onDragEnd: () => {
												setDragIndex(null);
												setOverIndex(null);
											}
										}}
										onTogglePin={(s) => void togglePin(s)}
										onDelete={setPendingDelete}
									/>
								))}
							</div>
							<h2 className="browse__section">全部</h2>
						</>
					)}
					<div className="source-list">
						{all.map((s) => (
							<SourceCard
								key={s.id}
								source={s}
								onTogglePin={(s) => void togglePin(s)}
								onDelete={setPendingDelete}
							/>
						))}
					</div>
				</>
			)}

			<Modal
				open={pendingDelete !== null}
				title={`卸载源 ${pendingDelete?.name ?? ''}？`}
				onClose={() => setPendingDelete(null)}
				footer={
					<>
						<Button onClick={() => setPendingDelete(null)}>取消</Button>
						<Button variant="danger" loading={del.isPending} onClick={() => void confirmDelete()}>
							卸载
						</Button>
					</>
				}
			>
				<p className="muted">已入库的漫画与下载不受影响。</p>
			</Modal>
		</div>
	);
}
