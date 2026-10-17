<script lang="ts">
	let { src, title, alt = '' }: { src: string | null | undefined; title: string; alt?: string } =
		$props();

	let failed = $state(false);
	let showImg = $derived(!!src && !failed);
</script>

{#if showImg}
	<img
		class="cover-img"
		src={src}
		alt={alt || title}
		loading="lazy"
		referrerpolicy="no-referrer"
		onerror={() => (failed = true)}
	/>
{:else}
	<div class="cover-placeholder"><span>{title ? title.slice(0, 6) : '无封面'}</span></div>
{/if}
