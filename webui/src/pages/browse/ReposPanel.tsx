import { useEffect, useMemo, useState } from 'react';
import { CircleArrowUp, Download, Plus, RefreshCw, Search, Trash } from 'lucide-react';
import {
	useAddSourceRepo,
	useDeleteSourceRepo,
	useInstallRepoSource,
	useRefreshSourceRepo,
	useRepoSources,
	useSourceRepos,
	useUpdateSource
} from '../../api/hooks';
import type { RepoSourceEntry, SourceRepo } from '../../api/types';
import { relativeTime } from '../../lib/format';
import {
	Button,
	Chip,
	EmptyState,
	IconButton,
	Input,
	Modal,
	Select,
	Skeleton,
	Switch,
	pushToast
} from '../../components';
import { iconColor } from './iconColor';

function repoTitle(repo: SourceRepo): string {
	if (repo.name) return repo.name;
	try {
		return new URL(repo.url).host;
	} catch {
		return repo.url;
	}
}

function EntryIcon({ entry }: { entry: RepoSourceEntry }) {
	const [failed, setFailed] = useState(false);
	if (!entry.iconUrl || failed) {
		return (
			<span className="source-card__icon-fallback" style={{ background: iconColor(entry.key) }}>
				{entry.name.slice(0, 1)}
			</span>
		);
	}
	return (
		<span className="source-card__icon">
			<img
				src={entry.iconUrl}
				alt=""
				loading="lazy"
				referrerPolicy="no-referrer"
				onError={() => setFailed(true)}
			/>
		</span>
	);
}

function EntryAction({ entry }: { entry: RepoSourceEntry }) {
	const install = useInstallRepoSource();
	const update = useUpdateSource();
	const busy = install.isPending || update.isPending;

	async function onClick() {
		try {
			if (entry.installedId !== null && entry.updateVersion !== null) {
				await update.mutateAsync(entry.installedId);
				pushToast(`已更新 ${entry.name} 到 v${entry.updateVersion}`, 'info');
			} else if (entry.installedId === null) {
				await install.mutateAsync({ repoId: entry.repoId, sourceKey: entry.key });
				pushToast(`已安装 ${entry.name}`, 'info');
			}
		} catch {
			// client 已弹错误 toast
		}
	}

	if (entry.installedId !== null && entry.updateVersion !== null) {
		return (
			<Button variant="primary" loading={busy} onClick={() => void onClick()}>
				<CircleArrowUp size={15} />
				更新 v{entry.updateVersion}
			</Button>
		);
	}
	if (entry.installedId !== null) {
		return <Button disabled>已装 v{entry.installedVersion}</Button>;
	}
	return (
		<Button variant="primary" loading={busy} onClick={() => void onClick()}>
			<Download size={15} />
			安装
		</Button>
	);
}

export default function ReposPanel() {
	const repos = useSourceRepos();
	const addRepo = useAddSourceRepo();
	const delRepo = useDeleteSourceRepo();
	const refreshRepo = useRefreshSourceRepo();
	const [url, setUrl] = useState('');
	const [pendingDelete, setPendingDelete] = useState<SourceRepo | null>(null);

	const [query, setQuery] = useState('');
	const [lang, setLang] = useState('');
	const [hideNsfw, setHideNsfw] = useState(false);

	// 搜索防抖 500ms，与源内搜索一致
	const [submitted, setSubmitted] = useState('');
	useEffect(() => {
		const t = setTimeout(() => setSubmitted(query.trim()), 500);
		return () => clearTimeout(t);
	}, [query]);

	const hasRepos = (repos.data ?? []).length > 0;
	const entries = useRepoSources(
		{ lang: lang || undefined, query: submitted || undefined, hideNsfw },
		hasRepos
	);
	// 语言候选从未过滤的全量派生，随过滤结果收缩会把选项自己筛没
	const allEntries = useRepoSources({}, hasRepos);
	const langOptions = useMemo(() => {
		const set = new Set<string>();
		for (const e of allEntries.data ?? []) {
			for (const l of e.languages) {
				const code = l.toLowerCase();
				if (code !== 'multi' && code !== 'all') set.add(code);
			}
		}
		return [...set].sort();
	}, [allEntries.data]);

	async function onAdd(e: React.FormEvent) {
		e.preventDefault();
		const trimmed = url.trim();
		if (!trimmed) return;
		try {
			const repo = await addRepo.mutateAsync(trimmed);
			pushToast(`已添加仓库 ${repoTitle(repo)}`, 'info');
			setUrl('');
		} catch {
			// client 已弹错误 toast
		}
	}

	async function onRefresh(repo: SourceRepo) {
		try {
			await refreshRepo.mutateAsync(repo.id);
			pushToast(`已刷新 ${repoTitle(repo)}`, 'info');
		} catch {
			// 失败时 repo 行的 fetchError 已落库，列表重拉后自带 stale 标
		}
	}

	async function confirmDelete() {
		if (!pendingDelete) return;
		try {
			await delRepo.mutateAsync(pendingDelete.id);
			pushToast(`已删除仓库 ${repoTitle(pendingDelete)}`, 'info');
			setPendingDelete(null);
		} catch {
			// client 已弹错误 toast
		}
	}

	return (
		<div className="repos">
			<form className="repos__add" onSubmit={(e) => void onAdd(e)}>
				<Input
					type="url"
					placeholder="源仓库地址，如 https://…/index.json"
					value={url}
					onChange={(e) => setUrl(e.currentTarget.value)}
				/>
				<Button variant="primary" type="submit" loading={addRepo.isPending}>
					<Plus size={15} />
					添加仓库
				</Button>
			</form>

			{repos.isPending ? (
				<div className="source-list">
					{Array.from({ length: 2 }, (_, i) => (
						<Skeleton key={i} height={64} style={{ borderRadius: 'var(--r-md)' }} />
					))}
				</div>
			) : !hasRepos ? (
				<EmptyState
					icon={Plus}
					title="还没有源仓库"
					hint="添加 Aidoku 兼容的源仓库地址后，就能浏览并一键安装里面的源"
				/>
			) : (
				<>
					<div className="source-list">
						{(repos.data ?? []).map((r) => (
							<div key={r.id} className="source-card">
								<div className="source-card__main">
									<div className="source-card__name">
										<span className="source-card__name-text">{repoTitle(r)}</span>
										{r.fetchError && <Chip color="danger">同步失败</Chip>}
									</div>
									<div className="source-card__meta">
										{r.url} · {r.sourceCount} 个源
										{r.fetchedAt ? ` · 更新于 ${relativeTime(r.fetchedAt)}` : ''}
									</div>
								</div>
								<IconButton
									label={`刷新 ${repoTitle(r)}`}
									onClick={() => void onRefresh(r)}
									disabled={refreshRepo.isPending}
								>
									<RefreshCw size={16} />
								</IconButton>
								<IconButton
									label={`删除 ${repoTitle(r)}`}
									variant="danger"
									onClick={() => setPendingDelete(r)}
								>
									<Trash size={17} />
								</IconButton>
							</div>
						))}
					</div>

					<div className="repos__filters">
						<Input
							icon={<Search size={15} />}
							type="search"
							placeholder="搜索源名字…"
							value={query}
							onChange={(e) => setQuery(e.currentTarget.value)}
						/>
						<Select value={lang} onChange={(e) => setLang(e.currentTarget.value)}>
							<option value="">全部语言</option>
							{langOptions.map((l) => (
								<option key={l} value={l}>
									{l}
								</option>
							))}
						</Select>
						<label className="repos__nsfw">
							<Switch checked={!hideNsfw} onChange={(on) => setHideNsfw(!on)} label="显示 NSFW" />
							显示 NSFW
						</label>
					</div>

					{entries.isPending ? (
						<div className="source-list">
							{Array.from({ length: 4 }, (_, i) => (
								<Skeleton key={i} height={82} style={{ borderRadius: 'var(--r-md)' }} />
							))}
						</div>
					) : (entries.data ?? []).length === 0 ? (
						<EmptyState icon={Search} title="没有匹配的源" hint="换个关键词或调整过滤条件试试" />
					) : (
						<div className="source-list">
							{(entries.data ?? []).map((entry) => (
								<div key={`${entry.repoId}:${entry.key}`} className="source-card">
									<EntryIcon entry={entry} />
									<div className="source-card__main">
										<div className="source-card__name">
											<span className="source-card__name-text">{entry.name}</span>
											{entry.contentRating > 1 && <Chip color="danger">NSFW</Chip>}
										</div>
										<div className="source-card__meta">
											{entry.key} · v{entry.version}
											{entry.repoName ? ` · ${entry.repoName}` : ''}
										</div>
										{entry.languages.length > 0 && (
											<div className="source-card__badges">
												{entry.languages.slice(0, 3).map((l) => (
													<Chip key={l}>{l}</Chip>
												))}
												{entry.languages.length > 3 && <Chip>+{entry.languages.length - 3}</Chip>}
											</div>
										)}
									</div>
									<EntryAction entry={entry} />
								</div>
							))}
						</div>
					)}
				</>
			)}

			<Modal
				open={pendingDelete !== null}
				title={`删除仓库 ${pendingDelete ? repoTitle(pendingDelete) : ''}？`}
				onClose={() => setPendingDelete(null)}
				footer={
					<>
						<Button onClick={() => setPendingDelete(null)}>取消</Button>
						<Button variant="danger" loading={delRepo.isPending} onClick={() => void confirmDelete()}>
							删除
						</Button>
					</>
				}
			>
				<p className="muted">已安装的源不受影响。</p>
			</Modal>
		</div>
	);
}
