<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import Toast from '#lib/components/Toast.svelte';

	let { children } = $props();

	const NAV = [
		{ href: '/library', label: '书库' },
		{ href: '/browse', label: '浏览' },
		{ href: '/history', label: '历史' },
		{ href: '/downloads', label: '下载' },
		{ href: '/updates', label: '更新' },
		{ href: '/settings', label: '设置' }
	];

	let menuOpen = $state(false);

	// 阅读器需要全屏，不显示全局导航
	let isReader = $derived(page.url.pathname.startsWith('/reader/'));

	function isActive(href: string): boolean {
		return page.url.pathname.startsWith(href);
	}
</script>

{#if !isReader}
<header class="nav">
	<div class="nav-inner">
		<a class="brand" href="/library">Suzuri</a>
		<nav class="links desktop">
			{#each NAV as item (item.href)}
				<a class:active={isActive(item.href)} href={item.href}>{item.label}</a>
			{/each}
		</nav>
		<button
			class="burger"
			aria-label="菜单"
			aria-expanded={menuOpen}
			onclick={() => (menuOpen = !menuOpen)}
		>
			{menuOpen ? '✕' : '☰'}
		</button>
	</div>
	{#if menuOpen}
		<nav class="links mobile">
			{#each NAV as item (item.href)}
				<a class:active={isActive(item.href)} href={item.href} onclick={() => (menuOpen = false)}>
					{item.label}
				</a>
			{/each}
		</nav>
	{/if}
</header>
{/if}

<main>
	{@render children()}
</main>

<Toast />

<style>
	.nav {
		position: sticky;
		top: 0;
		z-index: 50;
		background: color-mix(in srgb, var(--bg) 88%, transparent);
		backdrop-filter: blur(10px);
		border-bottom: 1px solid var(--border);
	}

	.nav-inner {
		max-width: 1080px;
		margin: 0 auto;
		height: var(--nav-h);
		display: flex;
		align-items: center;
		gap: 20px;
		padding: 0 16px;
	}

	.brand {
		font-weight: 700;
		font-size: 17px;
		color: var(--accent);
		letter-spacing: 0.5px;
	}

	.links {
		display: flex;
		gap: 4px;
	}

	.links a {
		padding: 6px 12px;
		border-radius: 8px;
		color: var(--text-dim);
		font-size: 14.5px;
	}

	.links a:hover {
		color: var(--text);
		background: var(--card);
	}

	.links a.active {
		color: var(--accent);
		font-weight: 600;
	}

	.burger {
		display: none;
		margin-left: auto;
		border: none;
		background: none;
		font-size: 18px;
		padding: 6px 10px;
	}

	.links.mobile {
		flex-direction: column;
		padding: 8px 16px 12px;
		border-top: 1px solid var(--border);
	}

	@media (max-width: 640px) {
		.links.desktop {
			display: none;
		}

		.burger {
			display: block;
		}
	}
</style>
