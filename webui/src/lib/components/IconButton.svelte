<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	interface Props extends HTMLButtonAttributes {
		/** 图标按钮没有可见文字，label 只做无障碍名称 */
		label: string;
		variant?: 'ghost' | 'danger';
		size?: 'sm' | 'md';
		children: Snippet;
	}

	let {
		label,
		variant = 'ghost',
		size = 'md',
		class: className = '',
		children,
		...rest
	}: Props = $props();
</script>

<button
	type="button"
	class={['icon-btn', variant === 'danger' && 'icon-btn--danger', size === 'sm' && 'icon-btn--sm', className]}
	aria-label={label}
	title={label}
	{...rest}
>
	{@render children()}
</button>
