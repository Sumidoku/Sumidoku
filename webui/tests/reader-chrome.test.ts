import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { INITIAL_READER_CHROME, isShortcutHelpKey, readerChromeReducer } from '../src/pages/reader/readerChrome.ts';
import type { ReaderChrome, ReaderChromeAction } from '../src/pages/reader/readerChrome.ts';

function run(state: ReaderChrome, ...actions: ReaderChromeAction[]): ReaderChrome {
	return actions.reduce(readerChromeReducer, state);
}

test('the bar starts visible with the menu closed', () => {
	assert.deepEqual(INITIAL_READER_CHROME, { overlay: true, settingsOpen: false, shortcutsOpen: false });
});

test('hiding the control bar closes the settings menu', () => {
	const open = run(INITIAL_READER_CHROME, 'toggleSettings');
	assert.deepEqual(open, { overlay: true, settingsOpen: true, shortcutsOpen: false });
	assert.deepEqual(run(open, 'toggleOverlay'), { overlay: false, settingsOpen: false, shortcutsOpen: false });
});

test('the menu stays closed when the bar comes back', () => {
	assert.deepEqual(run(INITIAL_READER_CHROME, 'toggleSettings', 'toggleOverlay', 'toggleOverlay'), {
		overlay: true,
		settingsOpen: false,
		shortcutsOpen: false
	});
});

test('the hidden settings button cannot open the menu behind the bar', () => {
	assert.deepEqual(run(INITIAL_READER_CHROME, 'toggleOverlay', 'toggleSettings', 'toggleOverlay'), {
		overlay: true,
		settingsOpen: false,
		shortcutsOpen: false
	});
});

test('the settings button and Escape still work while the bar is up', () => {
	assert.equal(run(INITIAL_READER_CHROME, 'toggleSettings', 'toggleSettings').settingsOpen, false);
	assert.equal(run(INITIAL_READER_CHROME, 'toggleSettings', 'closeSettings').settingsOpen, false);
});

test('shortcut help can be toggled while the toolbar is hidden and is exclusive with settings', () => {
	const open = run(INITIAL_READER_CHROME, 'toggleOverlay', 'toggleShortcuts');
	assert.deepEqual(open, { overlay: false, settingsOpen: false, shortcutsOpen: true });
	assert.deepEqual(run(open, 'toggleShortcuts').shortcutsOpen, false);
	assert.deepEqual(run(INITIAL_READER_CHROME, 'toggleSettings', 'toggleShortcuts'), {
		overlay: true,
		settingsOpen: false,
		shortcutsOpen: true
	});
	assert.deepEqual(run(INITIAL_READER_CHROME, 'toggleShortcuts', 'toggleSettings'), {
		overlay: true,
		settingsOpen: true,
		shortcutsOpen: false
	});
	assert.deepEqual(run(INITIAL_READER_CHROME, 'toggleShortcuts', 'toggleOverlay').shortcutsOpen, false);
});

test('H closes shortcut help even when the focused dialog is not a form control', () => {
	const key = { key: 'h', ctrlKey: false, altKey: false, metaKey: false };
	assert.equal(isShortcutHelpKey(key), true);
	assert.equal(isShortcutHelpKey({ ...key, key: 'm' }), false);
	assert.equal(isShortcutHelpKey({ ...key, ctrlKey: true }), false);
	assert.equal(isShortcutHelpKey({ ...key, metaKey: true }), false);
});

test('the reader keeps the bar and the menu in the one reducer', () => {
	const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /useReducer\(readerChromeReducer, INITIAL_READER_CHROME\)/);
	assert.doesNotMatch(source, /setOverlay|setSettingsOpen/);
	assert.match(source, /portalTarget=\{rootRef\.current \?\? undefined\}/);
	assert.match(source, /chaptersOpen \|\| shortcutsOpen/);
});
