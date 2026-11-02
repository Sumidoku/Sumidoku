import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import type { Dir, TapZoneLayout } from '../src/pages/reader/readerPrefs.ts';

test('tap previews cover the entire page ratio through the real zone resolver', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { TapZonePreview } = await vite.ssrLoadModule('/src/pages/reader/TapZonePreview.tsx');
		const { zoneAction, zoneAt } = await vite.ssrLoadModule('/src/pages/reader/readerPrefs.ts');
		for (const layout of ['lr', 'rl', 'edge', 'kindle', 'off'] as TapZoneLayout[]) {
			for (const dir of ['ltr', 'rtl'] as Dir[]) {
				const markup = renderToStaticMarkup(createElement(TapZonePreview, { layout, dir }));
				const cells = [...markup.matchAll(/<rect x="(\d+)" y="(\d+)" width="1" height="1" class="reader__tz-fill--(\w+)"><\/rect>/g)];
				assert.equal(cells.length, 20 * 28, `${layout} fills the preview without sampling a large grid`);
				assert.match(markup, /viewBox="0 0 20 28"/);
				for (const [, x, y, action] of cells) {
					assert.equal(zoneAction(dir, layout, zoneAt(layout, (Number(x) + 0.5) / 20, (Number(y) + 0.5) / 28)), action);
					assert(Number(x) < 20 && Number(y) < 28);
				}
				assert(cells.some(([, x, y]) => x === '19' && y === '27'), 'the bottom-right corner is painted');
			}
		}
	} finally {
		await vite.close();
	}
});
