import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import {
	DEFAULT_TYPOGRAPHY,
	loadTypography,
	saveTypography,
	typographyStyle
} from '../src/pages/reader/typography.ts';

const KEY = 'sumidoku.reader.typography';

function withStorage(run: (values: Map<string, string>) => void) {
	const values = new Map<string, string>();
	const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
	Object.defineProperty(globalThis, 'localStorage', {
		configurable: true,
		value: {
			getItem: (key: string) => values.get(key) ?? null,
			setItem: (key: string, value: string) => values.set(key, String(value))
		}
	});
	try {
		run(values);
	} finally {
		if (original) Object.defineProperty(globalThis, 'localStorage', original);
		else Reflect.deleteProperty(globalThis, 'localStorage');
	}
}

test('reader typography defaults and font stacks produce valid styles', () => {
	withStorage(() => {
		assert.deepEqual(loadTypography(), DEFAULT_TYPOGRAPHY);
		assert.deepEqual(typographyStyle(DEFAULT_TYPOGRAPHY), {
			fontFamily: undefined,
			fontSize: '1em',
			lineHeight: 1.8
		});
		for (const font of ['sans', 'serif', 'mono'] as const) {
			const style = typographyStyle({ ...DEFAULT_TYPOGRAPHY, font });
			assert.ok(style.fontFamily);
		}
		assert.match(typographyStyle({ ...DEFAULT_TYPOGRAPHY, font: 'serif' }).fontFamily!, /Songti SC/);
		assert.match(typographyStyle({ ...DEFAULT_TYPOGRAPHY, font: 'mono' }).fontFamily!, /monospace/);
	});
});

test('reader typography round-trips and rejects malformed stored values', () => {
	withStorage((values) => {
		const custom = { font: 'serif' as const, fontSize: 1.5, lineHeight: 2.2 };
		saveTypography(custom);
		assert.deepEqual(loadTypography(), custom);
		values.set(KEY, '{broken');
		assert.deepEqual(loadTypography(), DEFAULT_TYPOGRAPHY);
		values.set(KEY, JSON.stringify({ font: 'unknown', fontSize: 99, lineHeight: -1 }));
		assert.deepEqual(loadTypography(), DEFAULT_TYPOGRAPHY);
		values.set(KEY, JSON.stringify({ font: 'mono', fontSize: 2, lineHeight: 2.5 }));
		assert.deepEqual(loadTypography(), { font: 'mono', fontSize: 2, lineHeight: 2.5 });
	});
});

test('reader text paths apply typography and remeasure columns', () => {
	const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /style=\{\{\s*\.\.\.textStyle/);
	assert.match(source, /className="reader-text" style=\{textStyle\}/);
	assert.match(source, /\}, \[mode, idx, pages, typography\]\);/);
	assert.match(source, /reader\.typography/);
	assert.match(source, /reader\.fontFamily/);
});
