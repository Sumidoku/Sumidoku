<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLInputAttributes } from 'svelte/elements';

	interface Props extends HTMLInputAttributes {
		/** 左侧图标槽（搜索框放 Search 图标） */
		icon?: Snippet;
		value?: string;
	}
	let { icon, class: className = '', value = $bindable(''), ...rest }: Props = $props();
</script>

<span class={['input', icon && 'input--icon', className]}>
	{#if icon}<span class="input__icon">{@render icon()}</span>{/if}
	<input class="input__field" bind:value {...rest} />
</span>
