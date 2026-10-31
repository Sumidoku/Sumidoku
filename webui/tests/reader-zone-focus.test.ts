import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('every tap zone keeps focus out on a mouse press', () => {
	const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8');
	for (const zone of ['left', 'right', 'mid']) {
		assert.ok(
			source.includes(`className="reader__zone reader__zone--${zone}"\n\t\t\t\t\t\t\t\tonMouseDown={keepFocusOut}`),
			`${zone} zone`
		);
	}
});
