import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('updates settle from SSE summaries without polling or job-driven running state', () => {
	const source = readFileSync(new URL('../src/pages/UpdatesPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /sse\.on\('updateSummary'/);
	assert.match(source, /sse\.on\('updateComplete'/);
	assert.doesNotMatch(source, /setInterval\(/);
	const jobHandler = source.match(/\t\tfunction onJob[\s\S]*?\n\t\t\}/)?.[0];
	assert.ok(jobHandler);
	assert.doesNotMatch(jobHandler, /setSummary\(/);
});
