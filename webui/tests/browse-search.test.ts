import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

async function renderBrowse(path = '/browse'): Promise<string> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		await i18n.changeLanguage('en');
		const { default: BrowsePage } = await vite.ssrLoadModule('/src/pages/BrowsePage.tsx');
		return renderToStaticMarkup(
			createElement(
				QueryClientProvider,
				{ client: new QueryClient() },
				createElement(MemoryRouter, { initialEntries: [path] }, createElement(BrowsePage))
			)
		);
	} finally {
		await vite.close();
	}
}

test('browse keeps global search directly above the source views', async () => {
	const markup = await renderBrowse();
	assert.match(markup, /role="tab" aria-selected="true"[^>]*>Installed<\/button>/);
	assert.match(markup, /type="search"[^>]*placeholder="Search all sources…"/);
	assert.doesNotMatch(markup, /Cross-source search/);
});

test('aggregate filters are available before entering a query', async () => {
	const markup = await renderBrowse();
	assert.match(markup, />Rating</);
	assert.match(markup, />Language</);
	assert.match(markup, />Source</);
});

test('search deep link replaces the source tabs instead of nesting another tab', async () => {
	const markup = await renderBrowse('/browse?view=search');
	assert.match(markup, /type="search"[^>]*placeholder="Search all sources…"/);
	assert.match(markup, /Cross-source search/);
	assert.doesNotMatch(markup, /role="tab"/);
	assert.doesNotMatch(markup, /class="source-list"/);
});

test('search deep link restores the submitted query into the input', async () => {
	const markup = await renderBrowse('/browse?view=search&q=needle');
	assert.match(markup, /type="search"[^>]*value="needle"/);
});
