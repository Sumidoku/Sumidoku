<script lang="ts">
	import type { Snippet } from 'svelte';
	import { _ } from 'svelte-i18n';
	import Spinner from './Spinner.svelte';

	interface Props {
		hasNextPage: boolean;
		isFetching: boolean;
		onLoadMore: () => void;
		children: Snippet;
		endText?: string;
	}
	let { hasNextPage, isFetching, onLoadMore, children, endText }: Props = $props();

	let sentinel = $state<HTMLDivElement>();
	// IntersectionObserver 哨兵；读取最新 hasNextPage/isFetching 再触发（effect 随其变化重建）
	$effect(() => {
		const el = sentinel;
		if (!el) return;
		const ob = new IntersectionObserver((es) => {
			if (es[0].isIntersecting && hasNextPage && !isFetching) onLoadMore();
		});
		ob.observe(el);
		return () => ob.disconnect();
	});
</script>

{@render children()}
<div bind:this={sentinel} class="infinite-list__sentinel"></div>
<div class="infinite-list__footer">
	{#if isFetching}
		<Spinner />
	{:else if !hasNextPage}
		<span class="infinite-list__end">{endText ?? $_('list.end')}</span>
	{/if}
</div>
