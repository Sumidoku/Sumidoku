<script lang="ts">
	import type { Snippet } from 'svelte';
	type ChipColor = 'default' | 'accent' | 'danger' | 'success' | 'warning';

	interface Props {
		color?: ChipColor;
		/** 传了 onclick 就渲染成按钮（可点多选），否则是纯标签 */
		onclick?: () => void;
		title?: string;
		/** 追加到 chip 上的额外类名（如三态排除态 .chip--exclude） */
		class?: string;
		children: Snippet;
	}
	let { color = 'default', onclick, title, class: className = '', children }: Props = $props();
</script>

{#if onclick}
	<button
		type="button"
		class={['chip', color !== 'default' && `chip--${color}`, 'chip--clickable', className]}
		{title}
		{onclick}
	>
		{@render children()}
	</button>
{:else}
	<span class={['chip', color !== 'default' && `chip--${color}`, className]} {title}>
		{@render children()}
	</span>
{/if}
