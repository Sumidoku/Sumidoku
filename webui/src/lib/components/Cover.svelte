<script lang="ts">
	interface Props {
		src?: string | null;
		title: string;
		alt?: string;
	}
	let { src, title, alt }: Props = $props();

	let failed = $state(false);
	let loaded = $state(false);
	// 失败是按 src 记的：换封面这类原地换 src 的场景，旧图的失败不能锁死新图
	$effect(() => {
		src;
		failed = false;
		loaded = false;
	});
	const showImg = $derived(!!src && !failed);
</script>

<div class={['cover', showImg && !loaded && 'cover--shimmer']}>
	{#if showImg}
		<img
			class={['cover__img', loaded && 'cover__img--loaded']}
			src={src ?? undefined}
			alt={alt || title}
			loading="lazy"
			referrerpolicy="no-referrer"
			onload={() => (loaded = true)}
			onerror={() => (failed = true)}
		/>
	{:else}
		<div class="cover__placeholder">
			<span class="cover__ph-text">{title ? title.slice(0, 6) : '无封面'}</span>
			<span class="cover__ph-char" aria-hidden="true">{title ? title.slice(0, 1) : '无'}</span>
		</div>
	{/if}
</div>
