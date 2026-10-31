import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import type { ChapterWithDownload } from '../src/api/types.ts';

async function renderTransition(direction: 'next' | 'prev', withTarget = true): Promise<string> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		const previous = i18n.language;
		await i18n.changeLanguage('en');
		try {
			const { TransitionPage } = await vite.ssrLoadModule('/src/pages/reader/TransitionPage.tsx');
			const target = withTarget
				? ({
						id: 2,
						chapter_number: 2,
						title: 'The Next One',
						scanlators: ['Scan Group'],
						isRead: false,
						lastPageRead: 0
					} as unknown as ChapterWithDownload)
				: null;
			return renderToStaticMarkup(
				createElement(TransitionPage, {
					direction,
					currentLabel: 'Chapter 1',
					target,
					onContinue: () => {},
					onCancel: () => {}
				})
			);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
}

test('the next-chapter transition says the chapter finished and names the target', async () => {
	const markup = await renderTransition('next');
	assert.match(markup, /Chapter finished/);
	assert.match(markup, /Chapter 1/);
	assert.match(markup, /Next chapter/);
	assert.match(markup, /Scan Group/);
	assert.match(markup, /Continue to next chapter/);
	assert.match(markup, /Stay on this page/);
});

test('the prev-chapter transition says the chapter start and names the target', async () => {
	const markup = await renderTransition('prev');
	assert.match(markup, /At the chapter start/);
	assert.match(markup, /Previous chapter/);
	assert.match(markup, /Continue to previous chapter/);
});

test('the last-chapter transition reports no next chapter and offers to leave the reader', async () => {
	const markup = await renderTransition('next', false);
	assert.match(markup, /Chapter finished/);
	assert.match(markup, /No more chapters/);
	assert.doesNotMatch(markup, /Continue to next chapter/);
	assert.match(markup, /Back to the manga page/);
});

test('the first-chapter transition reports no previous chapter and offers to leave the reader', async () => {
	const markup = await renderTransition('prev', false);
	assert.match(markup, /At the chapter start/);
	assert.match(markup, /No previous chapter/);
	assert.doesNotMatch(markup, /Continue to previous chapter/);
	assert.match(markup, /Back to the manga page/);
});
