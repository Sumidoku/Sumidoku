<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Popover } from 'bits-ui';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import ChevronUp from '@lucide/svelte/icons/chevron-up';

	interface Props {
		label: string | Snippet;
		/** 启用态高亮 chip */
		active?: boolean;
		/** 拿到 close 以便点选后收起；多选场景不调用即保持菜单打开 */
		children: Snippet<[() => void]>;
	}
	let { label, active = false, children }: Props = $props();

	let open = $state(false);
	const close = () => (open = false);
</script>

<!-- bits-ui Popover：portal 到 body、浮动定位夹紧视口、Escape/外点收起，
     一举满足 LESSONS 里横向滚动 chip 栏下拉被裁剪/遮挡的修复要求 -->
<Popover.Root bind:open>
	<Popover.Trigger>
		{#snippet child({ props })}
			<button
				{...props}
				type="button"
				class={['chip', 'chip--clickable', active && 'chip--accent']}
			>
				{#if typeof label === 'string'}{label}{:else}{@render label()}{/if}
				{#if open}<ChevronUp size={13} />{:else}<ChevronDown size={13} />{/if}
			</button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content class="chip-dropdown__menu" align="start" sideOffset={4}>
		{@render children(close)}
	</Popover.Content>
</Popover.Root>
