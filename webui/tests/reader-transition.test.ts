import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import type { ChapterWithDownload } from '../src/api/types.ts';

// The transition page has no buttons: confirming is another same-direction
// turn, so the markup must carry the gesture hints instead of actions, and
// only the next-side page with a target owns the pull progress track
async function renderTransition(direction: 'next' | 'prev', withTarget = true, armed = false): Promise<string> {
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
					armed
				})
			);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
}

test('the next-chapter transition says the chapter finished, names the target and asks for the confirming turn', async () => {
	const markup = await renderTransition('next');
	assert.match(markup, /Chapter finished/);
	assert.match(markup, /Chapter 1/);
	assert.match(markup, /Next chapter/);
	assert.match(markup, /Scan Group/);
	assert.match(markup, /Turn again to continue/);
	assert.match(markup, /reader__pull-track/);
	assert.doesNotMatch(markup, /<button/);
});

test('the armed next-chapter transition announces the pull release', async () => {
	const markup = await renderTransition('next', true, true);
	assert.match(markup, /Release to open chapter 2/);
	assert.doesNotMatch(markup, /Turn again to continue/);
});

test('the prev-chapter transition says the chapter start and names the target, without a pull track', async () => {
	const markup = await renderTransition('prev');
	assert.match(markup, /At the chapter start/);
	assert.match(markup, /Previous chapter/);
	assert.match(markup, /Turn again to continue/);
	assert.doesNotMatch(markup, /reader__pull-track/);
	assert.doesNotMatch(markup, /<button/);
});

test('the last-chapter transition reports no next chapter and points at leaving the reader', async () => {
	const markup = await renderTransition('next', false);
	assert.match(markup, /Chapter finished/);
	assert.match(markup, /No more chapters/);
	assert.doesNotMatch(markup, /Turn again to continue/);
	assert.match(markup, /Turn again to leave the reader/);
	assert.doesNotMatch(markup, /reader__pull-track/);
});

test('the first-chapter transition reports no previous chapter and points at leaving the reader', async () => {
	const markup = await renderTransition('prev', false);
	assert.match(markup, /At the chapter start/);
	assert.match(markup, /No previous chapter/);
	assert.doesNotMatch(markup, /Turn again to continue/);
	assert.match(markup, /Turn again to leave the reader/);
});

test('an unparked transition page stays out of the accessibility tree', () => {
	// Both sides mount at the chapter ends before the track parks on them, so
	// their status regions must not be read out while off-screen
	const reader = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
	assert.match(reader, /'--transition-slot': -1 \} as CSSProperties\}[\s\S]{0,200}aria-hidden=\{pendingTurn !== 'prev'\}/);
	assert.match(reader, /'--transition-slot': spreads\.length \} as CSSProperties\}[\s\S]{0,200}aria-hidden=\{pendingTurn !== 'next'\}/);
});
