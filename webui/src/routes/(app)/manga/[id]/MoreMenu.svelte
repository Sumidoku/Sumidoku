<script lang="ts">
	import type { Snippet } from 'svelte';
	import { _ } from 'svelte-i18n';
	import EllipsisVertical from '@lucide/svelte/icons/ellipsis-vertical';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import { IconButton } from '$lib/components';

	// bits-ui 的 DropdownMenu 自带 portal，hero 容器 overflow:hidden 不再裁切弹层。
	// children 是渲染回调，拿到 close 以便某项操作完成后收起菜单（对齐 React 的 render-prop）。
	let { children }: { children: Snippet<[close: () => void]> } = $props();

	let open = $state(false);
	const close = () => (open = false);
</script>

<div class="manga-menu">
	<DropdownMenu.Root bind:open>
		<DropdownMenu.Trigger>
			{#snippet child({ props })}
				<IconButton {...props} label={$_('manga.moreActions')}>
					<EllipsisVertical size={17} />
				</IconButton>
			{/snippet}
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="end" class="manga-menu__content">
			{@render children(close)}
		</DropdownMenu.Content>
	</DropdownMenu.Root>
</div>
