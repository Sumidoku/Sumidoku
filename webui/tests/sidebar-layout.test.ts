import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
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
		// the assertions match English labels: force the language instead of inheriting the machine locale
		const { default: i18n } = await vite.ssrLoadModule('/src/i18n');
		await i18n.changeLanguage('en');
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

test('frequent destinations stay outside collapsible groups', async () => {
	const markup = await renderSidebar(false);
	const topLevel = markup.slice(markup.indexOf('<nav'), markup.indexOf('data-nav-group='));
	assert.deepEqual(
		[...topLevel.matchAll(/href="([^"]+)"/g)].map((match) => match[1]),
		['/library', '/browse', '/updates']
	);
});

test('sidebar groups start collapsed outside their routes', async () => {
	const markup = await renderSidebar(false);
	assert.equal(markup.match(/class="side-nav__children" hidden="" style="display:none"/g)?.length, 2);
	for (const group of ['common.nav.activity', 'common.nav.settings']) {
		assert.match(markup, new RegExp(`data-nav-group="${group}"[^>]*aria-expanded="false"`));
	}
});

test('settings deep link opens only the settings group and exposes section links', async () => {
	const markup = await renderSidebar(false, '/settings?section=downloads');
	assert.equal(markup.match(/class="side-nav__children" hidden="" style="display:none"/g)?.length, 1);
	assert.match(markup, /data-nav-group="common.nav.settings"[^>]*aria-expanded="true"/);
	assert.match(markup, /data-nav-group="common.nav.activity"[^>]*aria-expanded="false"/);
	assert.match(markup, /href="\/settings\?section=downloads"/);
	assert.match(markup, /href="\/settings\?section=trackers"/);
	const activeHrefs = [...markup.matchAll(/<a\b[^>]*>/g)]
		.filter(([anchor]) => anchor.includes('side-nav__item--active'))
		.map(([anchor]) => anchor.match(/href="([^"]+)"/)?.[1]);
	assert.deepEqual(activeHrefs, ['/settings?section=downloads']);
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

test('collapsing keeps the sidebar toggle at the same spot', async () => {
	const css = readFileSync(new URL('../src/layout/layout.css', import.meta.url), 'utf8');
	const rule = (selector: string) =>
		css.match(new RegExp(`${selector.replace(/[.]/g, '\\.')}\\s*\\{([^}]*)\\}`))?.[1] ?? '';
	// The toggle renders first, so a reversed column stacks the shell actions above it
	// and leaves it on the bottom row it occupies when expanded.
	const markup = await renderSidebar(true);
	const foot = markup.slice(markup.indexOf('app__sidebar-foot'));
	assert.ok(foot.indexOf('app__sidebar-toggle') >= 0);
	assert.ok(foot.indexOf('app__sidebar-toggle') < foot.indexOf('<span>Theme</span>'));
	assert.match(rule('.app__sidebar--collapsed .app__sidebar-foot'), /flex-direction:\s*column-reverse;/);
	// Collapsed, the 34px toggle centers in the 72px rail's 47px content box at 6.5px;
	// the expanded footer starts it at the same 6.5px so it does not shift sideways.
	assert.match(rule('.app__sidebar-foot'), /padding:\s*8px 4px 0 6\.5px;/);
});
