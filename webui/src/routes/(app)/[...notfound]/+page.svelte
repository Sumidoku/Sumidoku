<script lang="ts">
	import { _ } from 'svelte-i18n';
	import CircleQuestionMark from '@lucide/svelte/icons/circle-question-mark';
	import { Button } from '$lib/components/ui/button';
</script>

<div class="page container flex min-h-[60dvh] flex-col items-center justify-center gap-4 text-center">
	<CircleQuestionMark size={48} class="text-[var(--text-faint)]" />
	<h1 class="text-lg font-semibold">{$_('notFound.title')}</h1>
	<Button href="/library">{$_('notFound.back')}</Button>
</div>
