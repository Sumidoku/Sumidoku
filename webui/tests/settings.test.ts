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
});
