import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
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

const key = (k: string, extra: Record<string, unknown> = {}) => ({
	key: k,
	shiftKey: false,
	ctrlKey: false,
	altKey: false,
	metaKey: false,
	target: null,
	...extra
});

test('arrow keys turn pages only in paged mode', async () => {
	await withVite(async (vite) => {
		const { readerKeyAction } = await vite.ssrLoadModule('/src/pages/reader/hotkeys.ts');
		assert.equal(readerKeyAction(key('ArrowLeft'), 'paged'), 'left');
		assert.equal(readerKeyAction(key('ArrowRight'), 'paged'), 'right');
		assert.equal(readerKeyAction(key('ArrowLeft'), 'webtoon'), null);
		assert.equal(readerKeyAction(key('ArrowRight'), 'continuous'), null);
		// Vertical arrows scroll the continuous modes instead of doing nothing
		assert.equal(readerKeyAction(key('ArrowUp'), 'continuous'), 'scrollBack');
		assert.equal(readerKeyAction(key('ArrowDown'), 'webtoon'), 'scrollForward');
		assert.equal(readerKeyAction(key('ArrowUp'), 'paged'), null);
	});
});

test('page, home/end and space keys page in paged mode and scroll in continuous modes', async () => {
	await withVite(async (vite) => {
		const { readerKeyAction } = await vite.ssrLoadModule('/src/pages/reader/hotkeys.ts');
		assert.equal(readerKeyAction(key('PageUp'), 'paged'), 'pageBack');
		assert.equal(readerKeyAction(key('PageDown'), 'paged'), 'pageForward');
		assert.equal(readerKeyAction(key('PageUp'), 'webtoon'), 'scrollBack');
		assert.equal(readerKeyAction(key('PageDown'), 'continuous'), 'scrollForward');
		assert.equal(readerKeyAction(key('Home'), 'paged'), 'first');
		assert.equal(readerKeyAction(key('End'), 'continuous'), 'last');
		assert.equal(readerKeyAction(key(' '), 'paged'), 'pageForward');
		assert.equal(readerKeyAction(key(' ', { shiftKey: true }), 'paged'), 'pageBack');
		assert.equal(readerKeyAction(key(' '), 'webtoon'), 'scrollForward');
		assert.equal(readerKeyAction(key(' ', { shiftKey: true }), 'continuous'), 'scrollBack');
		assert.equal(readerKeyAction(key('m'), 'paged'), 'toggleOverlay');
		assert.equal(readerKeyAction(key('f'), 'paged'), 'toggleFullscreen');
		assert.equal(readerKeyAction(key('f'), 'webtoon'), 'toggleFullscreen');
		assert.equal(readerKeyAction(key('c'), 'paged'), 'cycleScale');
		assert.equal(readerKeyAction(key('c'), 'continuous'), null);
		assert.equal(readerKeyAction(key('x'), 'paged'), null);
	});
});

test('modifier chords and editable targets never reach the reader', async () => {
	await withVite(async (vite) => {
		const { readerKeyAction } = await vite.ssrLoadModule('/src/pages/reader/hotkeys.ts');
		assert.equal(readerKeyAction(key('ArrowLeft', { ctrlKey: true }), 'paged'), null);
		assert.equal(readerKeyAction(key('ArrowRight', { metaKey: true }), 'paged'), null);
		assert.equal(readerKeyAction(key('Home', { altKey: true }), 'paged'), null);
		// The settings sliders and the bottom page slider own their arrow keys
		const slider = { tagName: 'INPUT' } as unknown as EventTarget;
		assert.equal(readerKeyAction(key('ArrowLeft', { target: slider }), 'paged'), null);
		assert.equal(readerKeyAction(key('ArrowRight', { target: slider }), 'paged'), null);
		// Shift+Space stays a reader action: shift is not a browser chord
		assert.equal(readerKeyAction(key(' ', { shiftKey: true }), 'paged'), 'pageBack');
	});
});
