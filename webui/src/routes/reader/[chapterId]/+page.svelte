<script lang="ts">
	import { page } from '$app/state';
	import Reader from '#lib/components/Reader.svelte';

	let chapterId = $derived(Number(page.params.chapterId));
	let mangaId = $derived.by(() => {
		const v = page.url.searchParams.get('manga');
		return v === null ? null : Number(v);
	});
	let startPage = $derived.by(() => {
		const v = page.url.searchParams.get('page');
		return v === null ? null : Number(v);
	});
</script>

{#key chapterId}
	<Reader {chapterId} {mangaId} {startPage} />
{/key}
