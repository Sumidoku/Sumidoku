import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

test('login username starts empty', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		await i18n.changeLanguage('en');
		const { default: LoginPage } = await vite.ssrLoadModule('/src/pages/LoginPage.tsx');
		const markup = renderToStaticMarkup(createElement(MemoryRouter, null, createElement(LoginPage)));
		assert.match(markup, /<input[^>]*autoComplete="username"[^>]*value=""/);
		assert.doesNotMatch(markup, /<input[^>]*autoComplete="username"[^>]*value="admin"/);
	} finally {
		await vite.close();
	}
});
