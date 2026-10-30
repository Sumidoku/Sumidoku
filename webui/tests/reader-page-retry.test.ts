import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import {
	PAGE_AUTO_RETRIES,
	imageErrored,
	imageRetried,
	imageStateFor,
	retrySrc
} from '../src/pages/reader/pageRetry.ts';

const bare = '/api/v1/chapter/7/page/3';
const withWidth = '/api/v1/chapter/7/page/3?width=780';

test('the first attempt keeps the base page URL untouched', () => {
	assert.equal(retrySrc(bare, 0), bare);
	assert.equal(retrySrc(withWidth, 0), withWidth);
});

test('retrySrc picks the query separator from whether the base URL already has one', () => {
	assert.equal(retrySrc(bare, 1), '/api/v1/chapter/7/page/3?r=1');
	assert.equal(retrySrc(withWidth, 1), '/api/v1/chapter/7/page/3?width=780&r=1');
});

test('the first error arms exactly one automatic retry instead of latching', () => {
	assert.equal(PAGE_AUTO_RETRIES, 1);
	const state = imageErrored(null, bare);
	assert.deepEqual(state, { src: bare, attempt: 1, failed: false });
});

test('the second error latches the failure for the manual retry button', () => {
	const state = imageErrored(imageErrored(null, bare), bare);
	assert.deepEqual(state, { src: bare, attempt: 1, failed: true });
});

test('a manual retry clears the latch and reloads with a fresh cache-buster', () => {
	const latched = imageErrored(imageErrored(null, bare), bare);
	const retried = imageRetried(latched);
	assert.deepEqual(retried, { src: bare, attempt: 2, failed: false });
	assert.equal(retrySrc(retried.src, retried.attempt), '/api/v1/chapter/7/page/3?r=2');
});

test('a state stored for another page does not leak its failure or attempts into a new src', () => {
	const other = imageErrored(imageErrored(null, bare), bare);
	assert.deepEqual(imageStateFor(other, withWidth), { attempt: 0, failed: false });
	assert.deepEqual(imageErrored(other, withWidth), { src: withWidth, attempt: 1, failed: false });
});

test('PageImage initially renders the plain page URL with no retry UI', async () => {
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
			const { PageImage } = await vite.ssrLoadModule('/src/pages/reader/PageImage.tsx');
			const markup = renderToStaticMarkup(
				createElement(PageImage, { src: withWidth, alt: 'Page 4', className: 'reader__img' })
			);
			assert.match(markup, /<img/);
			assert.match(markup, /src="\/api\/v1\/chapter\/7\/page\/3\?width=780"/);
			assert.doesNotMatch(markup, /role="alert"/);
			assert.doesNotMatch(markup, /Retry/);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
});
