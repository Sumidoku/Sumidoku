import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { INITIAL_READER_CHROME, readerChromeReducer } from '../src/pages/reader/readerChrome.ts';
import type { ReaderChrome, ReaderChromeAction } from '../src/pages/reader/readerChrome.ts';

function run(state: ReaderChrome, ...actions: ReaderChromeAction[]): ReaderChrome {
	return actions.reduce(readerChromeReducer, state);
}

test('the bar starts visible with the menu closed', () => {
	assert.deepEqual(INITIAL_READER_CHROME, { overlay: true, settingsOpen: false });
});

test('hiding the control bar closes the settings menu', () => {
	const open = run(INITIAL_READER_CHROME, 'toggleSettings');
	assert.deepEqual(open, { overlay: true, settingsOpen: true });
	assert.deepEqual(run(open, 'toggleOverlay'), { overlay: false, settingsOpen: false });
});

test('the menu stays closed when the bar comes back', () => {
	assert.deepEqual(run(INITIAL_READER_CHROME, 'toggleSettings', 'toggleOverlay', 'toggleOverlay'), {
		overlay: true,
		settingsOpen: false
	});
});

test('the hidden settings button cannot open the menu behind the bar', () => {
	assert.deepEqual(run(INITIAL_READER_CHROME, 'toggleOverlay', 'toggleSettings', 'toggleOverlay'), {
		overlay: true,
		settingsOpen: false
	});
});

test('the settings button and Escape still work while the bar is up', () => {
	assert.equal(run(INITIAL_READER_CHROME, 'toggleSettings', 'toggleSettings').settingsOpen, false);
	assert.equal(run(INITIAL_READER_CHROME, 'toggleSettings', 'closeSettings').settingsOpen, false);
});

test('the reader keeps the bar and the menu in the one reducer', () => {
	const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /useReducer\(readerChromeReducer, INITIAL_READER_CHROME\)/);
	assert.doesNotMatch(source, /setOverlay|setSettingsOpen/);
});
