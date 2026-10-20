<script lang="ts">
	import '../app.css';
	import '$lib/styles/components.css';
	import '$lib/styles/pages.css';
	import { QueryClientProvider } from '@tanstack/svelte-query';
	import { queryClient } from '$lib/api/hooks';
	import { Toaster } from '$lib/components/ui/sonner';

	let { children } = $props();
</script>

<QueryClientProvider client={queryClient}>
	{@render children?.()}
</QueryClientProvider>

<Toaster position="top-center" />
