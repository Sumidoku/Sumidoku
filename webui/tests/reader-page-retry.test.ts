import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import {
	PAGE_AUTO_RETRIES,
	bumpRetries,
	failedInSpread,
	imageErrored,
	imageLoading,
	imageRetried,
	imageStateFor,
	retrySrc
} from '../src/pages/reader/pageRetry.ts';

const bare = '/api/v1/chapter/7/page/3';
const withQuery = '/api/v1/chapter/7/page/3?revision=8';

test('the first attempt keeps the base page URL untouched', () => {
	assert.equal(retrySrc(bare, 0), bare);
	assert.equal(retrySrc(withQuery, 0), withQuery);
});

test('retrySrc picks the query separator from whether the base URL already has one', () => {
	assert.equal(retrySrc(bare, 1), '/api/v1/chapter/7/page/3?r=1');
	assert.equal(retrySrc(withQuery, 1), '/api/v1/chapter/7/page/3?revision=8&r=1');
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
	assert.deepEqual(imageStateFor(other, withQuery), { attempt: 0, failed: false });
	assert.deepEqual(imageErrored(other, withQuery), { src: withQuery, attempt: 1, failed: false });
});

test('a page image with no recorded load is still loading', () => {
	assert.equal(imageLoading(null, bare, 0), true);
});

test('the recorded load clears the indicator only for the exact effective URL', () => {
	assert.equal(imageLoading(bare, bare, 0), false);
	assert.equal(imageLoading(withQuery, bare, 0), true);
});

test('an automatic retry re-arms the indicator: the cache-busted URL has not loaded yet', () => {
	assert.equal(imageLoading(bare, bare, 1), true);
	assert.equal(imageLoading('/api/v1/chapter/7/page/3?r=1', bare, 1), false);
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
				createElement(PageImage, { src: withQuery, alt: 'Page 4', className: 'reader__img' })
			);
			assert.match(markup, /<img/);
			assert.match(markup, /src="\/api\/v1\/chapter\/7\/page\/3\?revision=8"/);
			assert.doesNotMatch(markup, /role="alert"/);
			assert.doesNotMatch(markup, /Retry/);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
});

test('PageImage marks the image as loading until its own load event lands', async () => {
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
				createElement(PageImage, { src: bare, alt: 'Page 4', className: 'reader__img' })
			);
			assert.match(markup, /class="reader__img reader__img--loading"/);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
});

test('failedInSpread returns only the failed, non-null slots of the spread', () => {
	assert.deepEqual(failedInSpread([3, null], new Set([3])), [3]);
	assert.deepEqual(failedInSpread([3, 4], new Set([4, 9])), [4]);
	assert.deepEqual(failedInSpread([1, 2], new Set()), []);
});

test('bumpRetries increments each failed page and keeps the other entries', () => {
	assert.deepEqual(bumpRetries({}, [3]), { 3: 1 });
	assert.deepEqual(bumpRetries({ 3: 1, 5: 2 }, [3, 7]), { 3: 2, 5: 2, 7: 1 });
});

test('FailedPage offers the retry button in place by default', async () => {
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
			const { FailedPage } = await vite.ssrLoadModule('/src/pages/reader/PageImage.tsx');
			const markup = renderToStaticMarkup(
				createElement(FailedPage, { className: 'reader__img', onRetry: () => {} })
			);
			assert.match(markup, /role="alert"/);
			assert.match(markup, /<button/);
			assert.match(markup, /Retry/);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
});

test('FailedPage in overlay mode drops the in-slot button the tap zones would swallow', async () => {
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
			const { FailedPage } = await vite.ssrLoadModule('/src/pages/reader/PageImage.tsx');
			const markup = renderToStaticMarkup(
				createElement(FailedPage, { className: 'reader__img', retryInOverlay: true })
			);
			assert.match(markup, /role="alert"/);
			assert.doesNotMatch(markup, /<button/);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
});

test('the paged overlay retry sits above the tap zones', () => {
	const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8');
	const rule = css.match(/\.reader__page-retry\s*\{([^}]*)\}/)?.[1];
	assert.ok(rule);
	assert.match(rule, /\bposition:\s*absolute;/);
	assert.match(rule, /\bz-index:\s*[1-9]/);
});

test('the paged loading image keeps a box for the spinner while it has no intrinsic size', () => {
	const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8');
	const rule = css.match(/\.reader__paged \.reader__img--loading\s*\{([^}]*)\}/)?.[1];
	assert.ok(rule);
	assert.match(rule, /\bmin-width:\s*64px;/);
	assert.match(rule, /\bmin-height:\s*64px;/);
});
