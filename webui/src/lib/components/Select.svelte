<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLSelectAttributes } from 'svelte/elements';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';

	interface Props extends HTMLSelectAttributes {
		children: Snippet;
		value?: string | number;
	}
	let { children, class: className = '', value = $bindable(), ...rest }: Props = $props();
</script>

<span class={['select', className]}>
	<select class="select__field" bind:value {...rest}>{@render children()}</select>
	<span class="select__chevron"><ChevronDown size={14} /></span>
</span>
