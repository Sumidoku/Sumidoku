import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';

async function loadClamp() {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	const { clampMenuRight } = await vite.ssrLoadModule('/src/components/MenuButton.tsx');
	return { clampMenuRight, close: () => vite.close() };
}

test('a menu wider than the room left of its anchor stays inside the viewport', async () => {
	const { clampMenuRight, close } = await loadClamp();
	try {
		// The manga hero's ⋮ at 375px: anchor right edge 204.5, a 240px menu started at -35.5
		const right = clampMenuRight(375 - 204.5, 240, 375);
		assert.equal(375 - right - 240, 8);
	} finally {
		await close();
	}
});

test('a menu that fits keeps its right edge on the anchor', async () => {
	const { clampMenuRight, close } = await loadClamp();
	try {
		assert.equal(clampMenuRight(20, 240, 375), 20);
		assert.equal(clampMenuRight(127, 240, 375), 127);
	} finally {
		await close();
	}
});

test('every anchored menu goes through the viewport clamp', () => {
	for (const path of ['../src/components/MenuButton.tsx', '../src/pages/manga/MoreMenu.tsx', '../src/layout/LanguageMenu.tsx']) {
		const source = readFileSync(new URL(path, import.meta.url), 'utf8');
		assert.match(source, /useMenuInViewport\(open, btnRef, popRef, setPos/, path);
	}
});

test('the language menu measures its height instead of guessing', () => {
	// A hardcoded height estimate silently overflows the day the language list outgrows it (#619)
	const source = readFileSync(new URL('../src/layout/LanguageMenu.tsx', import.meta.url), 'utf8');
	assert.doesNotMatch(source, /\b\d{3,}\b(?=\s*[>*+])/);
	assert.doesNotMatch(source, /window\.innerHeight/);
});

test('the migrate row menu goes through MenuButton', () => {
	// The inline absolute popover had no flip, no portal and no outside-dismiss
	const page = readFileSync(new URL('../src/pages/migrate/MigratePage.tsx', import.meta.url), 'utf8');
	assert.match(page, /<MenuButton label=\{t\('migrate\.moreActions'\)\}>/);
	assert.doesNotMatch(page, /migrate-row__menu/);
	const styles = readFileSync(new URL('../src/pages/migrate/migrate.css', import.meta.url), 'utf8');
	assert.doesNotMatch(styles, /migrate-row__menu/);
});

test('a clamped menu scrolls instead of crossing the screen edge', () => {
	// The flip alone leaves a menu taller than its side of the viewport cut off; the position
	// clamp caps max-height and the stylesheet makes the menu scroll (#619)
	for (const [path, selector] of [
		['../src/components/components.css', '.menu-pop'],
		['../src/pages/manga.css', '.manga-menu__pop']
	] as const) {
		const source = readFileSync(new URL(path, import.meta.url), 'utf8');
		assert.match(source, new RegExp(`\\${selector} \\{[^}]*overflow-y: auto;`), path);
	}
});

test('menu offsets are measured without a classic scrollbar', () => {
	for (const path of [
		'../src/components/MenuButton.tsx',
		'../src/pages/manga/MoreMenu.tsx',
		'../src/components/useAnchoredMenu.ts'
	]) {
		const source = readFileSync(new URL(path, import.meta.url), 'utf8');
		assert.doesNotMatch(source, /window\.innerWidth(?! includes)/, path);
	}
});
