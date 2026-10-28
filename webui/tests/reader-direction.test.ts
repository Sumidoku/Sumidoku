import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';

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

test('the forward tap zone follows the reading direction', async () => {
	await withVite(async (vite) => {
		const { zoneMovesForward } = await vite.ssrLoadModule('/src/pages/reader/direction.ts');
		assert.equal(zoneMovesForward('ltr', 'right'), true);
		assert.equal(zoneMovesForward('ltr', 'left'), false);
		assert.equal(zoneMovesForward('rtl', 'left'), true);
		assert.equal(zoneMovesForward('rtl', 'right'), false);
	});
});

test('paged text columns translate with the reading direction', async () => {
	await withVite(async (vite) => {
		const { textColumnOffset } = await vite.ssrLoadModule('/src/pages/reader/direction.ts');
		assert.equal(textColumnOffset('ltr', 0, 500, 20), 0);
		assert.equal(textColumnOffset('ltr', 2, 500, 20), -1040);
		assert.equal(textColumnOffset('rtl', 0, 500, 20), 0);
		assert.equal(textColumnOffset('rtl', 2, 500, 20), 1040);
	});
});

test('paged text columns flow right-to-left under rtl', async () => {
	await withVite(async (vite) => {
		const { TextColumns } = await vite.ssrLoadModule('/src/pages/reader/TextColumns.tsx');
		const render = (dir: 'ltr' | 'rtl') =>
			renderToStaticMarkup(
				createElement(TextColumns, {
					dir,
					html: '<p>page</p>',
					textStyle: {},
					layout: { w: 500, gap: 20, cols: 3 },
					col: 1,
					innerRef: { current: null }
				})
			);
		const ltr = render('ltr');
		assert.match(ltr, /transform:translateX\(-520px\)/);
		assert.doesNotMatch(ltr, /direction:rtl/);
		const rtl = render('rtl');
		assert.match(rtl, /direction:rtl/);
		assert.match(rtl, /transform:translateX\(520px\)/);
	});
});

test('the bottom pager mirrors slider and chapter buttons under rtl', async () => {
	await withVite(async (vite) => {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		const previous = i18n.language;
		await i18n.changeLanguage('en');
		try {
			const { ReaderPager } = await vite.ssrLoadModule('/src/pages/reader/ReaderPager.tsx');
			const render = (dir: 'ltr' | 'rtl') =>
				renderToStaticMarkup(
					createElement(ReaderPager, {
						dir,
						hidden: false,
						navPos: 0,
						navTotal: 10,
						prevCh: null,
						nextCh: null,
						onSlider: () => {},
						onNavChapter: () => {}
					})
				);
			const ltr = render('ltr');
			assert.ok(ltr.indexOf('Previous chapter') < ltr.indexOf('Next chapter'), 'ltr keeps previous on the left');
			assert.doesNotMatch(ltr, /direction:rtl/);
			const rtl = render('rtl');
			assert.ok(rtl.indexOf('Next chapter') < rtl.indexOf('Previous chapter'), 'rtl puts next chapter on the leading left side');
			assert.match(rtl, /<input[^>]*style="direction:rtl"/);
		} finally {
			await i18n.changeLanguage(previous);
		}
	});
});
