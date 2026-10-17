<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '#lib/api';
	import type { Source } from '#lib/types';

	let sources = $state<Source[]>([]);
	let loading = $state(true);

	onMount(async () => {
		try {
			sources = await api<Source[]>('/sources');
		} finally {
			loading = false;
		}
	});
</script>

<div class="page">
	<h1 style="margin-bottom: 14px; font-size: 20px;">浏览源</h1>

	{#if loading}
		<div class="list">
			{#each Array(4) as _, i (i)}
				<div class="skeleton" style="height: 74px;"></div>
			{/each}
		</div>
	{:else if sources.length === 0}
		<div class="empty">
			<p>还没有安装任何源</p>
			<p class="faint">先把源包放进服务器的 sources 目录，再调用扫描接口</p>
		</div>
	{:else}
		<div class="list">
			{#each sources as s (s.id)}
				{#if s.enabled}
					<a class="src card" href="/browse/{s.id}">
						<div>
							<div class="name">{s.name}</div>
							<div class="meta faint">{s.key} · v{s.version}</div>
						</div>
						<span class="arrow">›</span>
					</a>
				{:else}
					<div class="src card disabled">
						<div>
							<div class="name">{s.name}</div>
							<div class="meta faint">{s.key} · v{s.version} · 已停用</div>
						</div>
					</div>
				{/if}
			{/each}
		</div>
	{/if}
</div>

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.src {
		display: flex;
		align-items: center;
		justify-content: space-between;
		transition: background 0.15s;
	}

	a.src:hover {
		background: var(--card-hover);
	}

	.src.disabled {
		opacity: 0.45;
	}

	.name {
		font-weight: 600;
	}

	.meta {
		font-size: 13px;
		margin-top: 2px;
	}

	.arrow {
		color: var(--text-faint);
		font-size: 20px;
	}
</style>
