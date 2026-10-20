<script lang="ts">
	import { iconColor } from './iconColor';

	interface Props {
		/** 图标地址；为空直接走色块回退 */
		src: string | null;
		name: string;
		/** 回退色块按此 key 取稳定底色 */
		fallbackKey: string;
	}
	let { src, name, fallbackKey }: Props = $props();

	let failed = $state(false);
</script>

{#if !src || failed}
	<span class="source-card__icon-fallback" style={`background:${iconColor(fallbackKey)}`}>
		{name.slice(0, 1)}
	</span>
{:else}
	<span class="source-card__icon">
		<img {src} alt="" loading="lazy" referrerpolicy="no-referrer" onerror={() => (failed = true)} />
	</span>
{/if}
