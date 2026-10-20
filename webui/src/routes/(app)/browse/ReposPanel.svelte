<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Globe from '@lucide/svelte/icons/globe';
	import Plus from '@lucide/svelte/icons/plus';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import Search from '@lucide/svelte/icons/search';
	import Trash from '@lucide/svelte/icons/trash';
	import {
		useAddSourceRepo,
		useDeleteSourceRepo,
		useRefreshSourceRepo,
		useRepoSources,
		useSourceRepos
	} from '$lib/api/hooks';
	import type { SourceRepo } from '$lib/api/types';
	import { relativeTime } from '$lib/format';
	import { loadRepoLangs, saveRepoLangs } from '$lib/repoPrefs';
	import {
		CONTENT_RATING_LEVELS,
		MULTI_LANGUAGE,
		compareLanguageLabel,
		contentRatingColor,
		contentRatingLabel,
		isMultiLanguage,
		languageLabel
	} from '$lib/source';
	import { Button, Chip, EmptyState, IconButton, Input, Modal, Skeleton, Switch, pushToast } from '$lib/components';
	import RepoEntryAction from './RepoEntryAction.svelte';
	import SourceIcon from './SourceIcon.svelte';

	const repos = useSourceRepos();
	const addRepo = useAddSourceRepo();
	const delRepo = useDeleteSourceRepo();
	const refreshRepo = useRefreshSourceRepo();

	let url = $state('');
	let pendingDelete = $state<SourceRepo | null>(null);
	let query = $state('');
	let langs = $state<string[]>(loadRepoLangs());
	let ratings = $state<number[]>(CONTENT_RATING_LEVELS.map((level) => level.value));
	let langOpen = $state(false);
	let submitted = $state('');

	function repoTitle(repo: SourceRepo): string {
		if (repo.name) return repo.name;
		try {
			return new URL(repo.url).host;
		} catch {
			return repo.url;
		}
	}

	// 搜索防抖 500ms，与源内搜索一致
	$effect(() => {
		const q = query;
		const t = setTimeout(() => (submitted = q.trim()), 500);
		return () => clearTimeout(t);
	});

	$effect(() => {
		saveRepoLangs(langs);
	});

	const hasRepos = $derived((repos.data ?? []).length > 0);
	const entries = useRepoSources(
		() => ({
			langs,
			query: submitted || undefined,
			// 三档全选等价于不过滤；不选任何档时传空串，服务端匹配空集
			contentRating:
				ratings.length === CONTENT_RATING_LEVELS.length ? undefined : ratings.join(',')
		}),
		() => hasRepos
	);
	// 语言候选从未过滤的全量派生，随过滤结果收缩会把选项自己筛没
	const allEntries = useRepoSources(
		() => ({}),
		() => hasRepos
	);
	const langOptions = $derived.by(() => {
		const set = new Set<string>();
		for (const e of allEntries.data ?? []) {
			for (const l of e.languages) {
				const code = l.trim().toLowerCase();
				if (code && !isMultiLanguage(code)) set.add(code);
			}
		}
		// 与 Aidoku 一致：多语言哨兵恒在列表首位
		return [MULTI_LANGUAGE, ...[...set].sort(compareLanguageLabel)];
	});

	function toggleLang(code: string) {
		langs = langs.includes(code) ? langs.filter((c) => c !== code) : [...langs, code];
	}

	function toggleRating(value: number) {
		ratings = ratings.includes(value)
			? ratings.filter((v) => v !== value)
			: [...ratings, value].sort((a, b) => a - b);
	}

	async function onAdd(e: SubmitEvent) {
		e.preventDefault();
		const trimmed = url.trim();
		if (!trimmed) return;
		try {
			const repo = await addRepo.mutateAsync(trimmed);
			pushToast($_('browse.repoAddedToast', { values: { name: repoTitle(repo) } }), 'info');
			url = '';
		} catch {
			// client 已弹错误 toast
		}
	}

	async function onRefresh(repo: SourceRepo) {
		try {
			await refreshRepo.mutateAsync(repo.id);
			pushToast($_('browse.repoRefreshedToast', { values: { name: repoTitle(repo) } }), 'info');
		} catch {
			// 失败时 repo 行的 fetchError 已落库，列表重拉后自带 stale 标
		}
	}

	async function confirmDelete() {
		if (!pendingDelete) return;
		try {
			await delRepo.mutateAsync(pendingDelete.id);
			pushToast($_('browse.repoDeletedToast', { values: { name: repoTitle(pendingDelete) } }), 'info');
			pendingDelete = null;
		} catch {
			// client 已弹错误 toast
		}
	}
</script>

<div class="repos">
	<form class="repos__add" onsubmit={(e) => void onAdd(e)}>
		<Input type="url" placeholder={$_('browse.repoUrlPlaceholder')} bind:value={url} />
		<Button variant="primary" type="submit" loading={addRepo.isPending}>
			<Plus size={15} />
			{$_('browse.addRepo')}
		</Button>
	</form>

	{#if repos.isPending}
		<div class="source-list">
			{#each Array.from({ length: 2 }) as _item, i (i)}
				<Skeleton height={64} />
			{/each}
		</div>
	{:else if !hasRepos}
		<EmptyState icon={Plus} title={$_('browse.reposEmptyTitle')} hint={$_('browse.reposEmptyHint')} />
	{:else}
		<div class="source-list">
			{#each repos.data ?? [] as r (r.id)}
				<div class="source-card">
					<div class="source-card__main">
						<div class="source-card__name">
							<span class="source-card__name-text">{repoTitle(r)}</span>
							{#if r.fetchError}<Chip color="danger">{$_('browse.repoSyncFailed')}</Chip>{/if}
						</div>
						<div class="source-card__meta">
							{r.url} · {$_('browse.repoSourceCount', { values: { n: r.sourceCount } })}{r.fetchedAt
								? ` · ${$_('browse.repoUpdatedAt', { values: { time: relativeTime(r.fetchedAt) } })}`
								: ''}
						</div>
					</div>
					<IconButton
						label={$_('browse.refreshLabel', { values: { name: repoTitle(r) } })}
						onclick={() => void onRefresh(r)}
						disabled={refreshRepo.isPending}
					>
						<RefreshCw size={16} />
					</IconButton>
					<IconButton
						label={$_('browse.deleteRepoLabel', { values: { name: repoTitle(r) } })}
						variant="danger"
						onclick={() => (pendingDelete = r)}
					>
						<Trash size={17} />
					</IconButton>
				</div>
			{/each}
		</div>

		<div class="repos__filters">
			<Input type="search" placeholder={$_('browse.searchPlaceholder')} bind:value={query}>
				{#snippet icon()}<Search size={15} />{/snippet}
			</Input>
			<Button onclick={() => (langOpen = true)}>
				<Globe size={15} />
				{langs.length > 0 ? $_('browse.languageCount', { values: { n: langs.length } }) : $_('browse.language')}
			</Button>
			<div class="repos__ratings" role="group" aria-label={$_('browse.ratingsGroup')}>
				{#each CONTENT_RATING_LEVELS as level (level.value)}
					{@const on = ratings.includes(level.value)}
					<Chip
						color={on ? level.color : 'default'}
						onclick={() => toggleRating(level.value)}
						title={on
							? $_('browse.ratingHide', { values: { label: level.label } })
							: $_('browse.ratingShow', { values: { label: level.label } })}
					>
						{level.label}
					</Chip>
				{/each}
			</div>
		</div>

		{#if entries.isPending}
			<div class="source-list">
				{#each Array.from({ length: 4 }) as _item, i (i)}
					<Skeleton height={82} />
				{/each}
			</div>
		{:else if (entries.data ?? []).length === 0}
			<EmptyState icon={Search} title={$_('browse.noMatchTitle')} hint={$_('browse.noMatchHint')} />
		{:else}
			<div class="source-list">
				{#each entries.data ?? [] as entry (`${entry.repoId}:${entry.key}`)}
					<div class="source-card">
						<SourceIcon src={entry.iconUrl} name={entry.name} fallbackKey={entry.key} />
						<div class="source-card__main">
							<div class="source-card__name">
								<span class="source-card__name-text">{entry.name}</span>
								{#if entry.contentRating > 0}
									<Chip color={contentRatingColor(entry.contentRating)}>
										{contentRatingLabel(entry.contentRating)}
									</Chip>
								{/if}
							</div>
							<div class="source-card__meta">
								{entry.key} · v{entry.version}{entry.repoName ? ` · ${entry.repoName}` : ''}
							</div>
							{#if entry.languages.length > 0}
								<div class="source-card__badges">
									{#each entry.languages.slice(0, 3) as l (l)}
										<Chip>{languageLabel(l)}</Chip>
									{/each}
									{#if entry.languages.length > 3}
										<Chip>+{entry.languages.length - 3}</Chip>
									{/if}
								</div>
							{/if}
						</div>
						<RepoEntryAction {entry} />
					</div>
				{/each}
			</div>
		{/if}
	{/if}

	<Modal open={langOpen} title={$_('browse.selectLanguage')} onClose={() => (langOpen = false)}>
		<div class="repos__lang-list">
			{#each langOptions as code (code)}
				<label class="repos__lang-row">
					<span>{languageLabel(code)}</span>
					<Switch
						checked={langs.includes(code)}
						onChange={() => toggleLang(code)}
						label={languageLabel(code)}
					/>
				</label>
			{/each}
		</div>
		{#snippet footer()}
			<Button variant="primary" onclick={() => (langOpen = false)}>{$_('browse.done')}</Button>
		{/snippet}
	</Modal>

	<Modal
		open={pendingDelete !== null}
		title={$_('browse.deleteRepoTitle', { values: { name: pendingDelete ? repoTitle(pendingDelete) : '' } })}
		onClose={() => (pendingDelete = null)}
	>
		<p class="muted">{$_('browse.deleteRepoBody')}</p>
		{#snippet footer()}
			<Button onclick={() => (pendingDelete = null)}>{$_('common.cancel')}</Button>
			<Button variant="danger" loading={delRepo.isPending} onclick={() => void confirmDelete()}>
				{$_('common.delete')}
			</Button>
		{/snippet}
	</Modal>
</div>
