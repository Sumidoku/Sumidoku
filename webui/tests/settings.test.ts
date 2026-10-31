import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';

test('structured settings merge preserves extra keys and nested values', async (context) => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	context.after(() => vite.close());
	const { mergeServerSettings } = await vite.ssrLoadModule('/src/api/hooks.ts');
	const current = {
		incognito: { enabled: false },
		library: {
			defaultCategory: 'all',
			showUncategorized: true,
			excludedUpdateCategories: [],
			skipTitles: ['hasUnread'],
			refreshMetadata: false,
			updateIntervalHours: 0
		},
		download: { movePath: null },
		backup: { enabled: false, intervalHours: 24, retentionCount: 10, lastRun: null, dir: 'backups' },
		browse: { pinnedSources: [] },
		cache: { chapterMaxBytes: 0 },
		'custom.option': { keep: true }
	};
	const merged = mergeServerSettings(current, {
		library: { refreshMetadata: true },
		backup: { dir: null }
	});
	assert.deepEqual(merged['custom.option'], { keep: true });
	assert.equal(merged.library.refreshMetadata, true);
	assert.deepEqual(merged.library.skipTitles, ['hasUnread']);
	assert.equal(merged.backup.dir, null);
	assert.equal(merged.backup.intervalHours, 24);
	assert.deepEqual(merged.cache, { chapterMaxBytes: 0 });
});

test('chapter cache budget accepts zero and small whole MiB without an accidental empty-input disable', async (context) => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)), configFile: false,
		logLevel: 'silent', appType: 'custom', server: { middlewareMode: true }
	});
	context.after(() => vite.close());
	const { chapterCacheLimitBytes } = await vite.ssrLoadModule('/src/pages/settings/chapterCache.ts');
	assert.equal(chapterCacheLimitBytes('0'), 0);
	assert.equal(chapterCacheLimitBytes('1'), 1024 ** 2);
	assert.equal(chapterCacheLimitBytes('255'), 255 * 1024 ** 2);
	for (const input of ['', ' ', '-1', '1.5', 'NaN', 'Infinity', '9007199254740991']) {
		assert.equal(chapterCacheLimitBytes(input), null, `invalid budget ${JSON.stringify(input)}`);
	}
});
