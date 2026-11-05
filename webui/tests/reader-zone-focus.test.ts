import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('every tap zone keeps focus out on a mouse press', () => {
	// Normalise line endings: a CRLF checkout would otherwise miss the \n in the needle
	const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
	for (const zone of ['left', 'right', 'mid']) {
		assert.ok(
			source.includes(`className="reader__zone reader__zone--${zone}"\n\t\t\t\t\t\t\t\tonMouseDown={keepFocusOut}`),
			`${zone} zone`
		);
	}
});
