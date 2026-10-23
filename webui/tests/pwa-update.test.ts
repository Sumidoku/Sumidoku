import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import { createPwaUpdateController } from '../src/pwa/updateController.ts';

test('update controller waits for confirmation and later never reloads', async () => {
	const controller = createPwaUpdateController();
	const calls: (boolean | undefined)[] = [];
	const updateSW = async (reloadPage?: boolean) => {
		calls.push(reloadPage);
	};

	controller.show(updateSW);
	assert.equal(controller.getSnapshot(), true);
	assert.deepEqual(calls, [], 'showing the prompt does not reload');
	controller.dismiss();
	assert.equal(controller.getSnapshot(), false);
	await controller.reload();
	assert.deepEqual(calls, [], 'dismissed updates cannot reload');

	controller.show(updateSW);
	await controller.reload();
	assert.deepEqual(calls, [true]);
	controller.dismiss();
});

test('update prompt has accessible controls and no virtual PWA import', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { default: i18n } = await vite.ssrLoadModule('/src/i18n');
		await i18n.changeLanguage('en');
		const { pwaUpdateController } = await vite.ssrLoadModule('/src/pwa/updateController.ts');
		const { PwaUpdatePrompt } = await vite.ssrLoadModule('/src/components/PwaUpdatePrompt.tsx');
		pwaUpdateController.show(async () => {});
		const markup = renderToStaticMarkup(createElement(PwaUpdatePrompt));
		assert.match(markup, /class="pwa-update" role="region" aria-label="Application update"/);
		assert.match(markup, /role="status"[^>]*>A new version is available\./);
		assert.match(markup, />Reload<\/button>/);
		assert.match(markup, />Later<\/button>/);
		assert.doesNotMatch(markup, /class="toaster/);
		await i18n.changeLanguage('zh-CN');
		const chineseMarkup = renderToStaticMarkup(createElement(PwaUpdatePrompt));
		assert.match(chineseMarkup, /aria-label="应用更新"/);
		assert.match(chineseMarkup, /新版本可用/);
		assert.match(chineseMarkup, />重新加载<\/button>/);
		assert.match(chineseMarkup, />稍后<\/button>/);
		pwaUpdateController.dismiss();
		assert.equal(renderToStaticMarkup(createElement(PwaUpdatePrompt)), '');
	} finally {
		await vite.close();
	}
});
