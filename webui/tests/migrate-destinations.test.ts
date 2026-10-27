import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test, { after, before } from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { Source } from '../src/api/types.ts';

let vite: ViteDevServer;

before(async () => {
	vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
	await i18n.changeLanguage('en');
});

after(() => vite.close());

function source(id: number, name: string): Source {
	return {
		id,
		key: `source-${id}`,
		name,
		kind: 'aidoku',
		version: 1,
		languages: ['en'],
		contentRating: 1,
		baseUrl: null,
		breakingChangeVersion: 0,
		config: null,
		enabled: true,
		installedAt: 1,
		updatedAt: 1,
		updateVersion: null,
		missingFromRepos: false,
		hasHome: null,
		pinned: false,
		pinOrder: null
	};
}

test('destination picker keeps the current source available for re-association', async () => {
	const client = new QueryClient();
	client.setQueryData(['sources'], [source(1, 'Example Source'), source(2, 'Other Source')]);
	const { DestinationPicker } = await vite.ssrLoadModule('/src/pages/migrate/DestinationPicker.tsx');
	const props = { selected: [], onChange: () => {}, excludeId: 1 };
	const markup = renderToStaticMarkup(
		createElement(QueryClientProvider, { client }, createElement(DestinationPicker, props))
	);
	assert.match(markup, /Example Source/);
	assert.match(markup, /Other Source/);
});

test('single and batch migration do not exclude the current source', async () => {
	const single = (await vite.ssrLoadModule('/src/pages/manga/MigrateModal.tsx?raw')).default;
	const batch = (await vite.ssrLoadModule('/src/pages/migrate/MigratePage.tsx?raw')).default;
	assert.doesNotMatch(single, /excludeId=/);
	assert.doesNotMatch(batch, /excludeId=/);
});
