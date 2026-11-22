import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

async function renderTabbar(path = '/library'): Promise<string> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { default: i18n } = await vite.ssrLoadModule('/src/i18n');
		await i18n.changeLanguage('en');
		const { MobileTabbar } = await vite.ssrLoadModule('/src/layout/MobileTabbar.tsx');
		return renderToStaticMarkup(createElement(MemoryRouter, { initialEntries: [path] }, createElement(MobileTabbar)));
	} finally {
		await vite.close();
	}
}

test('mobile tab bar mirrors the five desktop top-level destinations', async () => {
	const markup = await renderTabbar();
	assert.deepEqual(
		[...markup.matchAll(/href="([^"]+)"/g)].map((match) => match[1]),
		['/library', '/browse', '/updates', '/settings']
	);
	assert.equal(markup.match(/class="tabbar__item/g)?.length, 5);
	assert.match(markup, /<button[^>]*aria-expanded="false"[^>]*aria-controls="mobile-activity-menu"[^>]*aria-haspopup="menu"/);
	assert.match(markup, /<span class="tabbar__label">Activity</);
	assert.match(markup, /<span class="tabbar__label">Settings</);
	assert.doesNotMatch(markup, /role="menu"/);
});

test('activity stays selected on history and downloads routes', async () => {
	for (const path of ['/history', '/downloads']) {
		const markup = await renderTabbar(path);
		assert.match(markup, /<button[^>]*class="tabbar__item tabbar__item--active"[^>]*aria-current="page"/);
		assert.doesNotMatch(markup, /class="tabbar__activity-menu"/);
	}
});
