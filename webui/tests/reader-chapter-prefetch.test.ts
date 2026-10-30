import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import type { Page } from '../src/api/types.ts';

async function withVite<T>(run: (vite: ViteDevServer) => Promise<T>): Promise<T> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		return await run(vite);
	} finally {
		await vite.close();
	}
}

function page(content: Page['content']): Page {
	return { content, thumbnail: null, hasDescription: false, description: null, index: 0 };
}

test('pickProbePages keeps only probeable image pages, in order, capped', async () => {
	await withVite(async (vite) => {
		const { pickProbePages, PREFETCH_PAGE_COUNT } = await vite.ssrLoadModule(
			'/src/pages/reader/chapterPrefetch.ts'
		);
		const img = (i: number) => ({ ...page({ Url: [`https://example.invalid/${i}.png`, null] }), index: i });
		const text = (i: number) => ({ ...page({ Text: 'text' }), index: i });
		const unsupported = (i: number) => ({ ...page({ Image: 'blob.png' }), index: i });
		const pages = [img(0), text(1), img(2), unsupported(3), img(4), img(5), img(6), img(7)];
		const picked = pickProbePages(pages);
		// Text and unsupported pages are skipped; the cap keeps the first 4 probeable ones
		assert.deepEqual(picked.map((p) => p.index), [0, 2, 4, 5]);
		assert.equal(picked.length, PREFETCH_PAGE_COUNT);
	});
});

test('pickProbePages handles empty and null page lists', async () => {
	await withVite(async (vite) => {
		const { pickProbePages } = await vite.ssrLoadModule('/src/pages/reader/chapterPrefetch.ts');
		assert.deepEqual(pickProbePages(null), []);
		assert.deepEqual(pickProbePages([]), []);
		assert.deepEqual(pickProbePages(undefined), []);
	});
});
