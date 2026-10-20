<script lang="ts">
	import type { Snippet } from 'svelte';

	interface TabItem {
		key: string;
		label: string | Snippet;
		/** 角标计数（如分类下漫画数），null/undefined 不显示 */
		badge?: number | null;
	}
	interface Props {
		tabs: TabItem[];
		active: string;
		onChange: (key: string) => void;
	}
	let { tabs, active, onChange }: Props = $props();
</script>

<div class="tabs" role="tablist">
	{#each tabs as t (t.key)}
		<button
			type="button"
			role="tab"
			aria-selected={t.key === active}
			class={['tabs__tab', t.key === active && 'tabs__tab--active']}
			onclick={() => onChange(t.key)}
		>
			{#if typeof t.label === 'string'}{t.label}{:else}{@render t.label()}{/if}
			{#if t.badge != null}<span class="tabs__badge">{t.badge}</span>{/if}
		</button>
	{/each}
</div>
