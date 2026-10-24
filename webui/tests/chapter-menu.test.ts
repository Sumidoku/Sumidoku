import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';
import en from '../src/i18n/locales/en/chapterRow.json' with { type: 'json' };
import zhCN from '../src/i18n/locales/zh-CN/chapterRow.json' with { type: 'json' };

test('chapter menu consumes the row click before toggling', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { handleMenuToggle } = await vite.ssrLoadModule('/src/components/MenuButton.tsx');
		const calls: string[] = [];
		handleMenuToggle({ stopPropagation: () => calls.push('stop') }, () => calls.push('toggle'));
		assert.deepEqual(calls, ['stop', 'toggle']);
	} finally {
		await vite.close();
	}
});

test('mark-previous labels include every chapter below the current row', () => {
	assert.equal(en.chapterRow.markPreviousRead, 'Mark all chapters below as read');
	assert.equal(en.chapterRow.markPreviousUnread, 'Mark all chapters below as unread');
	assert.equal(zhCN.chapterRow.markPreviousRead, '标记下方所有章节为已读');
	assert.equal(zhCN.chapterRow.markPreviousUnread, '标记下方所有章节为未读');
});
