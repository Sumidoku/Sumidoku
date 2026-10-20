<script lang="ts">
	import { page } from '$app/state';
	import SourceBrowse from './SourceBrowse.svelte';
</script>

<!-- 每次 sourceId 变化强制整页重挂载：一次性初始化（存储筛选值、深链参数）
     随之重跑，对齐旧 React 路由的 key={sourceId} 行为 -->
{#key page.params.sourceId}
	<SourceBrowse sourceId={Number(page.params.sourceId)} />
{/key}
