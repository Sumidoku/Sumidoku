import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';
import { readSidebarCollapsed, writeSidebarCollapsed } from '../src/lib/sidebarPrefs.ts';

async function renderSidebar(collapsed: boolean, path = '/'): Promise<string> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		await vite.ssrLoadModule('/src/i18n');
		const { DesktopSidebar } = await vite.ssrLoadModule('/src/layout/DesktopSidebar.tsx');
		return renderToStaticMarkup(
			createElement(
				MemoryRouter,
				{ initialEntries: [path] },
				createElement(DesktopSidebar, { collapsed, onToggle: () => {} }, createElement('span', null, 'Theme'))
			)
		);
	} finally {
		await vite.close();
	}
}

test('collapsed sidebar keeps accessible icon navigation', async () => {
	const markup = await renderSidebar(true);
	assert.match(markup, /class="app__sidebar app__sidebar--collapsed"/);
	assert.match(markup, /class="app__logo-text" hidden="">Sumidoku/);
	assert.match(markup, /class="side-nav__label" hidden="">Library/);
	assert.match(markup, /aria-label="Library"/);
	assert.match(markup, /aria-label="Primary navigation"/);
	assert.match(markup, /aria-label="Expand sidebar"/);
	assert.match(markup, /aria-expanded="false"/);
	assert.match(markup, /aria-controls="desktop-sidebar"/);
});

test('expanded sidebar keeps visible labels and collapse control', async () => {
	const markup = await renderSidebar(false);
	assert.doesNotMatch(markup, /app__sidebar--collapsed/);
	assert.match(markup, /class="app__logo-text">Sumidoku/);
	assert.match(markup, /class="side-nav__label">Library/);
	assert.match(markup, /aria-label="Collapse sidebar"/);
	assert.match(markup, /aria-expanded="true"/);
});

test('sidebar groups start collapsed outside their routes', async () => {
	const markup = await renderSidebar(false);
	assert.equal(markup.match(/class="side-nav__children" hidden="" style="display:none"/g)?.length, 3);
	for (const group of ['common.nav.library', 'common.nav.downloads', 'common.nav.settings']) {
		assert.match(markup, new RegExp(`data-nav-group="${group}"[^>]*aria-expanded="false"`));
	}
});

test('settings deep link opens only the settings group and exposes section links', async () => {
	const markup = await renderSidebar(false, '/settings?section=downloads');
	assert.equal(markup.match(/class="side-nav__children" hidden="" style="display:none"/g)?.length, 2);
	assert.match(markup, /data-nav-group="common.nav.settings"[^>]*aria-expanded="true"/);
	assert.match(markup, /data-nav-group="common.nav.library"[^>]*aria-expanded="false"/);
	assert.match(markup, /data-nav-group="common.nav.downloads"[^>]*aria-expanded="false"/);
	assert.match(markup, /href="\/settings\?section=downloads"/);
});

test('sidebar preference round-trips and rejects unknown values', () => {
	const values = new Map<string, string>();
	const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
	Object.defineProperty(globalThis, 'localStorage', {
		configurable: true,
		value: {
			getItem: (key: string) => values.get(key) ?? null,
			setItem: (key: string, value: string) => values.set(key, String(value))
		}
	});
	try {
		assert.equal(readSidebarCollapsed(), false);
		writeSidebarCollapsed(true);
		assert.equal(values.get('sumidoku.sidebar.collapsed'), 'true');
		assert.equal(readSidebarCollapsed(), true);
		writeSidebarCollapsed(false);
		assert.equal(readSidebarCollapsed(), false);
		values.set('sumidoku.sidebar.collapsed', 'unexpected');
		assert.equal(readSidebarCollapsed(), false);
	} finally {
		if (original) Object.defineProperty(globalThis, 'localStorage', original);
		else Reflect.deleteProperty(globalThis, 'localStorage');
	}
});
