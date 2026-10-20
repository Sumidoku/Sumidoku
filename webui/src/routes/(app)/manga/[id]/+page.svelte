<script lang="ts">
	import { page } from '$app/state';
	import '$lib/styles/manga.css';
	import MangaDetail from './MangaDetail.svelte';
</script>

<!-- The route component is reused across id changes; key on the param so all
     per-manga state (prefs, selection, download flags, modals) resets. -->
{#key page.params.id}
	<MangaDetail id={Number(page.params.id)} />
{/key}
