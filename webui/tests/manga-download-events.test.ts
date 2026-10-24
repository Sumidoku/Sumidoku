import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('manga detail reconciles terminal download events', () => {
	const source = readFileSync(new URL('../src/pages/MangaDetailPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /subscribeWS\('\/api\/v1\/downloads\/ws'/);
	assert.match(source, /m.type !== 'finished' && m.type !== 'error'/);
	assert.match(source, /next.delete\(m.chapterId\)/);
	assert.match(source, /c.id === m.chapterId \? \{ \.\.\.c, isDownloaded: true \}/);
	assert.match(source, /queryKey: qk.library/);
});
