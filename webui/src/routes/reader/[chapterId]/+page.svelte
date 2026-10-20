<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { page } from '$app/state';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { EmptyState } from '$lib/components';
	import '$lib/styles/reader.css';
	import ReaderCore from './ReaderCore.svelte';

	const chapterId = $derived(Number(page.params.chapterId));
	const mangaRaw = $derived(page.url.searchParams.get('manga'));
	const pageRaw = $derived(page.url.searchParams.get('page'));
	const mangaId = $derived(
		mangaRaw !== null && Number.isFinite(Number(mangaRaw)) ? Number(mangaRaw) : null
	);
	const startPage = $derived(
		pageRaw !== null && Number.isFinite(Number(pageRaw)) ? Number(pageRaw) : null
	);
</script>

{#if !Number.isFinite(chapterId)}
	<div class="reader reader--msg">
		<EmptyState icon={TriangleAlert} title={$_('reader.invalidChapter')} />
	</div>
{:else}
	<!-- key 钉在 chapterId 上，切章整体重挂载，页码/上报队列/observer 随之重置 -->
	{#key chapterId}
		<ReaderCore {chapterId} {mangaId} {startPage} />
	{/key}
{/if}
