<script lang="ts">
	import { toastState } from '#lib/toast.svelte';
</script>

<div class="toast-wrap" aria-live="polite">
	{#each toastState.items as t (t.id)}
		<div class="toast" class:info={t.kind === 'info'}>{t.message}</div>
	{/each}
</div>

<style>
	.toast-wrap {
		position: fixed;
		left: 50%;
		bottom: 24px;
		transform: translateX(-50%);
		display: flex;
		flex-direction: column;
		gap: 8px;
		z-index: 200;
		width: min(420px, calc(100vw - 32px));
		pointer-events: none;
	}

	.toast {
		background: color-mix(in srgb, var(--danger) 16%, var(--card));
		border: 1px solid color-mix(in srgb, var(--danger) 45%, transparent);
		color: var(--text);
		border-radius: var(--radius);
		padding: 10px 14px;
		font-size: 14px;
		animation: rise 0.18s ease-out;
	}

	.toast.info {
		background: color-mix(in srgb, var(--accent) 14%, var(--card));
		border-color: var(--accent-dim);
	}

	@keyframes rise {
		from {
			opacity: 0;
			transform: translateY(8px);
		}
	}
</style>
