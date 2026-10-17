<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api';
	import Cover from '#lib/components/Cover.svelte';
	import type { Filter, FilterValue, Listing, Manga, MangaPage, Source } from '#lib/types';

	const sourceId = Number(page.params.sourceId);

	type CheckVal = 0 | 1 | -1;

	let source = $state<Source | null>(null);
	let filters = $state<Filter[] | null>(null);
	let listings = $state<Listing[] | null>(null);
	let showFilters = $state(false);

	let mode = $state<'search' | string>('search');
	let query = $state('');

	// 各过滤控件的 UI 状态，按 kind 分别存放
	let textVals = $state<Record<string, string>>({});
	let sortVals = $state<Record<string, { index: number | null; ascending: boolean }>>({});
	let checkVals = $state<Record<string, CheckVal>>({});
	let selectVals = $state<Record<string, string>>({});
	let multiVals = $state<Record<string, Record<string, CheckVal>>>({});
	let rangeVals = $state<Record<string, { from: string; to: string }>>({});

	let entries = $state<Manga[]>([]);
	let pageNo = $state(1);
	let hasNext = $state(false);
	let loading = $state(false);
	let initial = $state(true);
	let sentinel = $state<HTMLDivElement | null>(null);

	function initFilterState(fs: Filter[]) {
		for (const f of fs) {
			if ('Text' in f.kind) {
				textVals[f.id] = '';
			} else if ('Sort' in f.kind) {
				const k = f.kind.Sort;
				const di = k.default ? k.options.indexOf(k.default) : -1;
				sortVals[f.id] = { index: di >= 0 ? di : null, ascending: true };
			} else if ('Check' in f.kind) {
				checkVals[f.id] = f.kind.Check.default ? 1 : 0;
			} else if ('Select' in f.kind) {
				selectVals[f.id] = f.kind.Select.default ?? '';
			} else if ('MultiSelect' in f.kind) {
				const k = f.kind.MultiSelect;
				const rec: Record<string, CheckVal> = {};
				for (const v of k.default_included ?? []) rec[v] = 1;
				for (const v of k.default_excluded ?? []) rec[v] = -1;
				multiVals[f.id] = rec;
			} else if ('Range' in f.kind) {
				rangeVals[f.id] = { from: '', to: '' };
			}
		}
	}

	// MultiSelect 选中值取 ids（有 ids 时）否则取选项名
	function multiKey(f: Filter, i: number): string {
		const k = 'MultiSelect' in f.kind ? f.kind.MultiSelect : null;
		if (!k) return '';
		return k.ids?.[i] ?? k.options[i];
	}

	function selectKey(f: Filter, i: number): string {
		const k = 'Select' in f.kind ? f.kind.Select : null;
		if (!k) return '';
		return k.ids?.[i] ?? k.options[i];
	}

	function buildFilterValues(): FilterValue[] {
		const out: FilterValue[] = [];
		for (const f of filters ?? []) {
			if ('Text' in f.kind) {
				const v = (textVals[f.id] ?? '').trim();
				if (v) out.push({ Text: { id: f.id, value: v } });
			} else if ('Sort' in f.kind) {
				const s = sortVals[f.id];
				if (s && s.index !== null) {
					out.push({ Sort: { id: f.id, index: s.index, ascending: s.ascending } });
				}
			} else if ('Check' in f.kind) {
				const v = checkVals[f.id] ?? 0;
				if (v !== 0) out.push({ Check: { id: f.id, value: v } });
			} else if ('Select' in f.kind) {
				const v = selectVals[f.id] ?? '';
				if (v) out.push({ Select: { id: f.id, value: v } });
			} else if ('MultiSelect' in f.kind) {
				const rec = multiVals[f.id] ?? {};
				const included = Object.entries(rec)
					.filter(([, v]) => v === 1)
					.map(([k]) => k);
				const excluded = Object.entries(rec)
					.filter(([, v]) => v === -1)
					.map(([k]) => k);
				if (included.length || excluded.length) {
					out.push({ MultiSelect: { id: f.id, included, excluded } });
				}
			} else if ('Range' in f.kind) {
				const r = rangeVals[f.id];
				if (!r) continue;
				const from = r.from.trim() === '' ? null : Number(r.from);
				const to = r.to.trim() === '' ? null : Number(r.to);
				if ((from !== null && !Number.isNaN(from)) || (to !== null && !Number.isNaN(to))) {
					out.push({ Range: { id: f.id, from, to } });
				}
			}
		}
		return out;
	}

	async function fetchPage(p: number) {
		loading = true;
		try {
			let data: MangaPage;
			if (mode === 'search') {
				data = await api<MangaPage>(`/sources/${sourceId}/search`, {
					method: 'POST',
					body: { query: query.trim() || undefined, page: p, filters: buildFilterValues() }
				});
			} else {
				data = await api<MangaPage>(
					`/sources/${sourceId}/listings/${encodeURIComponent(mode)}?page=${p}`
				);
			}
			entries = p === 1 ? data.entries : [...entries, ...data.entries];
			hasNext = data.hasNextPage;
			pageNo = p;
		} finally {
			loading = false;
			initial = false;
		}
	}

	function runSearch() {
		entries = [];
		fetchPage(1);
	}

	function switchMode(m: string) {
		if (m === mode) return;
		mode = m;
		entries = [];
		initial = true;
		fetchPage(1);
	}

	function cycleMulti(f: Filter, i: number) {
		const k = 'MultiSelect' in f.kind ? f.kind.MultiSelect : null;
		if (!k) return;
		const key = multiKey(f, i);
		const rec = { ...(multiVals[f.id] ?? {}) };
		const cur = rec[key] ?? 0;
		// 可排除时三态循环：未选 → 包含 → 排除 → 未选
		const next: CheckVal = cur === 0 ? 1 : cur === 1 ? (k.can_exclude ? -1 : 0) : 0;
		if (next === 0) delete rec[key];
		else rec[key] = next;
		multiVals[f.id] = rec;
	}

	async function openEntry(m: Manga) {
		if (m.id) {
			goto(`/manga/${m.id}`);
			return;
		}
		const { id } = await api<{ id: number }>('/manga', {
			method: 'POST',
			body: { sourceId, mangaKey: m.key }
		});
		goto(`/manga/${id}`);
	}

	onMount(async () => {
		const [srcs, fs, ls] = await Promise.all([
			api<Source[]>('/sources').catch(() => null),
			api<Filter[]>(`/sources/${sourceId}/filters`, { silent: true }).catch(() => null),
			api<Listing[]>(`/sources/${sourceId}/listings`, { silent: true }).catch(() => null)
		]);
		source = srcs?.find((s) => s.id === sourceId) ?? null;
		filters = fs && fs.length > 0 ? fs : null;
		if (filters) initFilterState(filters);
		listings = ls && ls.length > 0 ? ls : null;
		await fetchPage(1);
	});

	$effect(() => {
		if (!sentinel) return;
		const ob = new IntersectionObserver((es) => {
			if (es[0].isIntersecting && hasNext && !loading) fetchPage(pageNo + 1);
		});
		ob.observe(sentinel);
		return () => ob.disconnect();
	});
</script>

<div class="page">
	<h1 style="margin-bottom: 12px; font-size: 20px;">{source?.name ?? '源'}</h1>

	{#if listings}
		<div class="tabs" style="margin-bottom: 12px;">
			<button class="tab" class:active={mode === 'search'} onclick={() => switchMode('search')}>
				搜索
			</button>
			{#each listings as l (l.id)}
				<button class="tab" class:active={mode === l.id} onclick={() => switchMode(l.id)}>
					{l.name}
				</button>
			{/each}
		</div>
	{/if}

	{#if mode === 'search'}
		<form
			class="search-row"
			onsubmit={(e) => {
				e.preventDefault();
				runSearch();
			}}
		>
			<input
				type="search"
				placeholder="搜索漫画标题…"
				bind:value={query}
				style="flex: 1; min-width: 0;"
			/>
			<button class="primary" type="submit">搜索</button>
			{#if filters}
				<button type="button" onclick={() => (showFilters = !showFilters)}>
					筛选{showFilters ? ' ▴' : ' ▾'}
				</button>
			{/if}
		</form>

		{#if filters && showFilters}
			<div class="card filter-panel">
				{#each filters as f (f.id)}
					{#if 'Note' in f.kind}
						<div class="f-item">
							<div class="f-title">{f.title}</div>
							<p class="muted" style="margin: 0; font-size: 13.5px;">{f.kind.Note}</p>
						</div>
					{:else if 'Text' in f.kind}
						<div class="f-item">
							<label class="f-title" for="f-{f.id}">{f.title}</label>
							<input
								id="f-{f.id}"
								type="text"
								placeholder={f.kind.Text.placeholder ?? ''}
								bind:value={textVals[f.id]}
							/>
						</div>
					{:else if 'Sort' in f.kind}
						<div class="f-item">
							<label class="f-title" for="f-{f.id}">{f.title}</label>
							<div style="display: flex; gap: 8px;">
								<select
									id="f-{f.id}"
									style="flex: 1;"
									value={sortVals[f.id]?.index ?? ''}
									onchange={(e) => {
										const v = e.currentTarget.value;
										sortVals[f.id] = {
											index: v === '' ? null : Number(v),
											ascending: sortVals[f.id]?.ascending ?? true
										};
									}}
								>
									<option value="">默认</option>
									{#each f.kind.Sort.options as opt, i (i)}
										<option value={i}>{opt}</option>
									{/each}
								</select>
								{#if f.kind.Sort.can_ascend && sortVals[f.id]?.index !== null}
									<button
										type="button"
										onclick={() => {
											const s = sortVals[f.id];
											if (s) sortVals[f.id] = { ...s, ascending: !s.ascending };
										}}
									>
										{sortVals[f.id]?.ascending ? '升序' : '降序'}
									</button>
								{/if}
							</div>
						</div>
					{:else if 'Check' in f.kind}
						<div class="f-item">
							<div class="f-title">{f.title}{f.kind.Check.name ? ` · ${f.kind.Check.name}` : ''}</div>
							{#if f.kind.Check.can_exclude}
								<div class="seg">
									{#each [['不选', 0], ['包含', 1], ['排除', -1]] as [label, v] (v)}
										<button
											type="button"
											class="seg-btn"
											class:on={checkVals[f.id] === v}
											onclick={() => (checkVals[f.id] = v as CheckVal)}>{label}</button
										>
									{/each}
								</div>
							{:else}
								<button
									type="button"
									class="chip clickable"
									class:on={checkVals[f.id] === 1}
									onclick={() => (checkVals[f.id] = checkVals[f.id] === 1 ? 0 : 1)}
								>
									{checkVals[f.id] === 1 ? '已选' : '未选'}
								</button>
							{/if}
						</div>
					{:else if 'Select' in f.kind}
						<div class="f-item">
							<label class="f-title" for="f-{f.id}">{f.title}</label>
							<select id="f-{f.id}" bind:value={selectVals[f.id]}>
								<option value="">未选</option>
								{#each f.kind.Select.options as opt, i (i)}
									<option value={selectKey(f, i)}>{opt}</option>
								{/each}
							</select>
						</div>
					{:else if 'MultiSelect' in f.kind}
						<div class="f-item">
							<div class="f-title">{f.title}</div>
							<div class="chips">
								{#each f.kind.MultiSelect.options as opt, i (i)}
									{@const st = multiVals[f.id]?.[multiKey(f, i)] ?? 0}
									<button
										type="button"
										class="chip clickable"
										class:on={st === 1}
										class:off={st === -1}
										title={f.kind.MultiSelect.can_exclude
											? '点击切换：包含 / 排除 / 不选'
											: '点击切换选中'}
										onclick={() => cycleMulti(f, i)}
									>
										{st === -1 ? '− ' : ''}{opt}
									</button>
								{/each}
							</div>
						</div>
					{:else if 'Range' in f.kind}
						<div class="f-item">
							<div class="f-title">{f.title}</div>
							<div style="display: flex; gap: 8px; align-items: center;">
								<input
									type="number"
									placeholder={f.kind.Range.min != null ? String(f.kind.Range.min) : '最小'}
									step={f.kind.Range.decimal ? 'any' : '1'}
									bind:value={rangeVals[f.id].from}
									style="width: 100%;"
								/>
								<span class="faint">—</span>
								<input
									type="number"
									placeholder={f.kind.Range.max != null ? String(f.kind.Range.max) : '最大'}
									step={f.kind.Range.decimal ? 'any' : '1'}
									bind:value={rangeVals[f.id].to}
									style="width: 100%;"
								/>
							</div>
						</div>
					{/if}
				{/each}
				<div style="display: flex; justify-content: flex-end; margin-top: 4px;">
					<button class="primary" type="button" onclick={runSearch}>应用筛选</button>
				</div>
			</div>
		{/if}
	{/if}

	{#if initial}
		<div class="manga-grid" style="margin-top: 14px;">
			{#each Array(10) as _, i (i)}
				<div>
					<div class="skeleton" style="aspect-ratio: 3/4;"></div>
					<div class="skeleton" style="height: 14px; margin-top: 8px;"></div>
				</div>
			{/each}
		</div>
	{:else if entries.length === 0}
		<div class="empty">没有结果</div>
	{:else}
		<div class="manga-grid" style="margin-top: 14px;">
			{#each entries as m, i (m.key + i)}
				<button class="manga-card as-btn" onclick={() => openEntry(m)}>
					<Cover src={m.cover} title={m.title} />
					<div class="title">{m.title}</div>
				</button>
			{/each}
		</div>
		<div bind:this={sentinel} style="height: 1px;"></div>
		{#if loading}
			<div style="text-align: center; padding: 16px;"><span class="spin"></span></div>
		{/if}
	{/if}
</div>

<style>
	.search-row {
		display: flex;
		gap: 8px;
		margin-bottom: 12px;
	}

	.filter-panel {
		display: flex;
		flex-direction: column;
		gap: 14px;
		margin-bottom: 4px;
	}

	.f-title {
		display: block;
		font-size: 13.5px;
		color: var(--text-dim);
		margin-bottom: 6px;
	}

	.f-item input,
	.f-item select {
		width: 100%;
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.seg {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	.seg-btn {
		border: none;
		border-radius: 0;
		background: var(--bg);
		padding: 6px 14px;
		font-size: 13.5px;
		color: var(--text-dim);
	}

	.seg-btn.on {
		background: var(--accent);
		color: #14161c;
		font-weight: 600;
	}

	.manga-card.as-btn {
		border: none;
		background: none;
		padding: 0;
		text-align: left;
		cursor: pointer;
		display: block;
		width: 100%;
	}
</style>
