import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

test('library exposes category strips and a dedicated responsive tools region', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		await vite.ssrLoadModule('/src/i18n');
		const { default: LibraryPage } = await vite.ssrLoadModule('/src/pages/LibraryPage.tsx');
		const markup = renderToStaticMarkup(
			createElement(
				QueryClientProvider,
				{ client: new QueryClient() },
				createElement(MemoryRouter, null, createElement(LibraryPage))
			)
		);
		assert.match(markup, /class="library__filters library__categories"/);
		assert.match(markup, /class="library__category-chips"/);
		assert.match(markup, /<\/div><\/div><div class="library__filters library__filters--status">/);
		assert.match(markup, /class="library__tools"/);
		assert.match(markup, /aria-label="Sort"/);
	} finally {
		await vite.close();
	}
});
