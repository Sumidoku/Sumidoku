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
	for (const path of ['../src/components/MenuButton.tsx', '../src/pages/manga/MoreMenu.tsx']) {
		const source = readFileSync(new URL(path, import.meta.url), 'utf8');
		assert.match(source, /useMenuInViewport\(open, popRef, setPos\);/, path);
	}
});

test('menu offsets are measured without a classic scrollbar', () => {
	for (const path of ['../src/components/MenuButton.tsx', '../src/pages/manga/MoreMenu.tsx']) {
		const source = readFileSync(new URL(path, import.meta.url), 'utf8');
		assert.doesNotMatch(source, /window\.innerWidth(?! includes)/, path);
	}
});
