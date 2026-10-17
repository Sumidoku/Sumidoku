<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		open,
		title,
		onclose,
		children
	}: { open: boolean; title: string; onclose: () => void; children: Snippet } = $props();
</script>

{#if open}
	<div class="mask" role="presentation" onclick={onclose}>
		<div class="panel" role="presentation" onclick={(e) => e.stopPropagation()}>
			<div class="head">
				<h3>{title}</h3>
				<button class="x" onclick={onclose} aria-label="关闭">✕</button>
			</div>
			<div class="body">
				{@render children()}
			</div>
		</div>
	</div>
{/if}

<style>
	.mask {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.6);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 100;
		padding: 16px;
	}

	.panel {
		background: var(--bg-raised);
		border: 1px solid var(--border);
		border-radius: 12px;
		width: min(480px, 100%);
		max-height: 80vh;
		display: flex;
		flex-direction: column;
	}

	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 14px 16px;
		border-bottom: 1px solid var(--border);
	}

	.x {
		border: none;
		background: none;
		padding: 4px 8px;
		color: var(--text-dim);
	}

	.body {
		padding: 14px 16px;
		overflow-y: auto;
	}
</style>
