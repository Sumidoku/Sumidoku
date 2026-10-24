import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

const source = {
	id: 2,
	key: 'zh.test',
	name: 'Test Source',
	version: 1,
	installedAt: 0,
	updatedAt: 0,
	baseUrl: null,
	breakingChangeVersion: 0,
	config: null,
	contentRating: 1,
	enabled: true,
	hasHome: true,
	kind: 'aidoku',
	languages: ['zh'],
	missingFromRepos: false,
	pinOrder: null,
	pinned: false,
	updateVersion: 2
};

async function renderSource(overrides = {}) {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		// the assertions match English aria labels: force the language instead of inheriting the machine locale
		const { default: i18n } = await vite.ssrLoadModule('/src/i18n');
		await i18n.changeLanguage('en');
		const { SourceCard } = await vite.ssrLoadModule('/src/pages/BrowsePage.tsx');
		return renderToStaticMarkup(
			createElement(
				QueryClientProvider,
				{ client: new QueryClient() },
				createElement(
					MemoryRouter,
					null,
					createElement(SourceCard, {
						source: { ...source, ...overrides },
						onTogglePin: () => {},
						onToggleEnabled: () => {},
						onDelete: () => {}
					})
				)
			)
		);
	} finally {
		await vite.close();
	}
}

test('source card keeps desktop actions and exposes a mobile more trigger', async () => {
	const markup = await renderSource();
	assert.match(markup, /class="source-card__actions"/);
	assert.match(markup, /class="source-card__mobile-actions"/);
	assert.match(markup, /aria-label="Actions for Test Source"/);
	assert.match(markup, /aria-label="Uninstall Test Source"/);
});

test('local source card exposes pin, settings and uninstall but no enable toggle', async () => {
	const markup = await renderSource({ id: 1, key: 'local.local', name: 'Local', kind: 'local', hasHome: null, updateVersion: null, contentRating: 0, languages: [] });
	assert.match(markup, /source-card__mobile-actions/);
	assert.match(markup, /aria-label="Pin Local"/);
	assert.match(markup, /aria-label="Uninstall Local"/);
	assert.doesNotMatch(markup, /aria-label="Disable Local"/);
	assert.doesNotMatch(markup, /aria-label="Home for Local"/);
});

test('drag handle stays after card content instead of shifting the leading column', () => {
	const css = readFileSync(new URL('../src/pages/pages.css', import.meta.url), 'utf8');
	const rule = css.match(/\.source-card__grip\s*\{([^}]*)\}/);
	assert.ok(rule);
	assert.match(rule[1], /\border:\s*1;/);
});
