<script lang="ts">
	let { value }: { value?: number | null } = $props();
	const pct = $derived(
		value != null && Number.isFinite(value) ? Math.min(100, Math.max(0, value * 100)) : null
	);
</script>

<div class="progress" role="progressbar" aria-valuenow={pct ?? undefined}>
	{#if pct !== null}
		<div class="progress__fill" style={`width:${pct}%`}></div>
	{:else}
		<div class="progress__fill progress__fill--indeterminate"></div>
	{/if}
</div>
