<script lang="ts">
	import type { HomeComponentValue, HomeLink } from '$lib/api/types';

	type ImageScrollerValue = Extract<HomeComponentValue, { ImageScroller: unknown }>['ImageScroller'];

	interface Props {
		value: ImageScrollerValue;
		onOpenLink: (link: HomeLink) => void;
	}
	let { value, onOpenLink }: Props = $props();

	const links = $derived(value.links);
	const ratio = $derived(
		value.width && value.height && value.width > 0 && value.height > 0
			? `${value.width} / ${value.height}`
			: undefined
	);

	let rowEl: HTMLDivElement | null = $state(null);
	let paused = $state(false);

	// auto_scroll_interval（秒）驱动的横向自动滚动：每次前进到下一项，到头回卷；指针悬停暂停
	$effect(() => {
		const interval = value.auto_scroll_interval;
		if (!interval || interval <= 0 || links.length < 2 || paused) return;
		const id = window.setInterval(() => {
			const el = rowEl;
			if (!el) return;
			// 子项 offsetLeft 以 .home-hrow（position:relative）为参照，与 scrollLeft 同坐标系
			const next = Array.from(el.children).find(
				(c) => (c as HTMLElement).offsetLeft > el.scrollLeft + 8
			) as HTMLElement | undefined;
			el.scrollTo({ left: next ? next.offsetLeft : 0, behavior: 'smooth' });
		}, interval * 1000);
		return () => window.clearInterval(id);
	});
</script>

<!-- 悬停暂停自动滚动：纯装饰性 hover，不需要键盘可达 -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="home-hrow"
	bind:this={rowEl}
	onpointerenter={() => (paused = true)}
	onpointerleave={() => (paused = false)}
>
	{#each links as link, i (i)}
		<button
			type="button"
			class="home-banner"
			style={ratio ? `aspect-ratio:${ratio}` : undefined}
			onclick={() => onOpenLink(link)}
		>
			{#if link.image_url}
				<img src={link.image_url} alt={link.title} loading="lazy" referrerpolicy="no-referrer" />
				<span class="home-banner__caption">
					<span class="home-banner__caption-title">{link.title}</span>
					{#if link.subtitle}<span class="home-banner__caption-sub">{link.subtitle}</span>{/if}
				</span>
			{:else}
				<span class="home-banner__fallback">{link.title}</span>
			{/if}
		</button>
	{/each}
</div>
