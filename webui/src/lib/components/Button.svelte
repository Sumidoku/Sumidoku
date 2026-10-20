<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import Spinner from './Spinner.svelte';

	interface Props extends HTMLButtonAttributes {
		variant?: 'primary' | 'ghost' | 'danger';
		size?: 'sm' | 'md';
		loading?: boolean;
		children: Snippet;
	}

	let {
		variant = 'ghost',
		size = 'md',
		loading = false,
		disabled = false,
		class: className = '',
		children,
		...rest
	}: Props = $props();
</script>

<button
	type="button"
	class={[
		'btn',
		variant === 'primary' && 'btn--primary',
		variant === 'danger' && 'btn--danger',
		size === 'sm' && 'btn--sm',
		className
	]}
	disabled={disabled || loading}
	{...rest}
>
	{#if loading}<Spinner />{/if}
	{@render children()}
</button>
