<script lang="ts">
	import type { Snippet } from 'svelte';
	import Cover from './Cover.svelte';
	import Spinner from './Spinner.svelte';

	interface Props {
		src?: string | null;
		title: string;
		subtitle?: string | Snippet;
		/** >0 时在封面右上角显示未读角标 */
		unreadCount?: number;
		/** 二选一：内部跳转走 to，自定义行为（如先注册再跳）走 onclick */
		to?: string;
		onclick?: () => void;
		/** 点击后异步跳转中置灰防连点 */
		busy?: boolean;
	}
	let { src, title, subtitle, unreadCount, to, onclick, busy }: Props = $props();
</script>

{#snippet inner()}
	<div class="cover-card__cover">
		<Cover {src} {title} />
		{#if unreadCount != null && unreadCount > 0}
			<span class="cover-card__badge">{unreadCount > 99 ? '99+' : unreadCount}</span>
		{/if}
		{#if busy}
			<div class="cover-card__busy"><Spinner /></div>
		{/if}
	</div>
	<div class="cover-card__title">{title}</div>
	{#if typeof subtitle === 'string'}
		<div class="cover-card__subtitle">{subtitle}</div>
	{:else if subtitle}
		<div class="cover-card__subtitle">{@render subtitle()}</div>
	{/if}
{/snippet}

{#if to}
	<a class={['cover-card', busy && 'cover-card--busy']} href={to}>{@render inner()}</a>
{:else}
	<button type="button" class={['cover-card', busy && 'cover-card--busy']} {onclick} disabled={busy}>
		{@render inner()}
	</button>
{/if}
