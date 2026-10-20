<script lang="ts">
	interface Props {
		width?: number | string;
		height?: number | string;
		/** 例 '2 / 3'，用于封面位骨架 */
		aspect?: string;
		class?: string;
	}
	let { width, height, aspect, class: className = '' }: Props = $props();
	const dim = (v: number | string | undefined) => (typeof v === 'number' ? `${v}px` : v);
	const style = $derived(
		[
			width != null && `width:${dim(width)}`,
			height != null && `height:${dim(height)}`,
			aspect && `aspect-ratio:${aspect}`
		]
			.filter(Boolean)
			.join(';')
	);
</script>

<div class={['skeleton', className]} {style}></div>
