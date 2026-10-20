<script lang="ts">
	import { _ } from 'svelte-i18n';
	let { size }: { size?: number } = $props();
</script>

<span
	class="spinner"
	style={size ? `width:${size}px;height:${size}px` : undefined}
	role="status"
	aria-label={$_('common.loading')}
></span>
