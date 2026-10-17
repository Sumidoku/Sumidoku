import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { PackageOpen, ScanSearch, Trash } from 'lucide-react';
import { sourceIconUrl } from '../api/client';
import { useDeleteSource, useScanSources, useSources } from '../api/hooks';
import type { Source } from '../api/types';
import {
	Button,
	Chip,
	EmptyState,
	IconButton,
	Modal,
	PageHeader,
	Skeleton,
	pushToast
} from '../components';

// 图标加载失败的色块底色：按 key hash 从 8 色板取，同源每次渲染颜色一致
const ICON_COLORS = [
	'#7c6cf0',
	'#4f8ef7',
	'#3fb68b',
	'#d9a03f',
	'#e5534b',
	'#c86dd7',
	'#5fb8a6',
	'#8b9cf9'
];

function iconColor(key: string): string {
	let h = 0;
	for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) | 0;
	return ICON_COLORS[Math.abs(h) % ICON_COLORS.length];
}

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

export default function BrowsePage() {
	const navigate = useNavigate();
	const sources = useSources();
	const scan = useScanSources();
	const del = useDeleteSource();
	const [pendingDelete, setPendingDelete] = useState<Source | null>(null);

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
					<Button variant="primary" loading={scan.isPending} onClick={() => void onScan()}>
						<ScanSearch size={15} />
						扫描源目录
					</Button>
				}
			/>

			{sources.isPending ? (
				<div className="source-list">
					{Array.from({ length: 4 }, (_, i) => (
						<Skeleton key={i} height={82} style={{ borderRadius: 'var(--r-md)' }} />
					))}
				</div>
			) : (sources.data ?? []).length === 0 ? (
				<EmptyState
					icon={PackageOpen}
					title="还没有安装源"
					hint="把源包放进服务器的 sources 目录，然后点右上角「扫描源目录」"
				/>
			) : (
				<div className="source-list">
					{(sources.data ?? []).map((s) => (
						<div
							key={s.id}
							className={
								s.enabled ? 'source-card source-card--link' : 'source-card source-card--disabled'
							}
							role={s.enabled ? 'link' : undefined}
							tabIndex={s.enabled ? 0 : undefined}
							onClick={() => s.enabled && navigate(`/browse/${s.id}`)}
							onKeyDown={(e) => {
								if (s.enabled && (e.key === 'Enter' || e.key === ' ')) {
									e.preventDefault();
									navigate(`/browse/${s.id}`);
								}
							}}
						>
							<SourceIcon source={s} />
							<div className="source-card__main">
								<div className="source-card__name">
									<span className="source-card__name-text">{s.name}</span>
									{s.contentRating > 1 && <Chip color="danger">NSFW</Chip>}
								</div>
								<div className="source-card__meta">
									{s.key} · v{s.version}
									{!s.enabled ? ' · 已停用' : ''}
								</div>
								{s.languages.length > 0 && (
									<div className="source-card__badges">
										{s.languages.slice(0, 3).map((lang) => (
											<Chip key={lang}>{lang}</Chip>
										))}
										{s.languages.length > 3 && <Chip>+{s.languages.length - 3}</Chip>}
									</div>
								)}
							</div>
							<IconButton
								label={`卸载 ${s.name}`}
								variant="danger"
								onClick={(e) => {
									e.stopPropagation();
									setPendingDelete(s);
								}}
							>
								<Trash size={17} />
							</IconButton>
						</div>
					))}
				</div>
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
