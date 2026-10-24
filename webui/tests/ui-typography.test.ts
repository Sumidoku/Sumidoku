import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

function read(path: string) {
	return readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
}

test('system font tokens cover sans, display, brand and mono roles', () => {
	const tokens = read('src/styles/tokens.css');
	assert.match(tokens, /--font-sans:/);
	assert.match(tokens, /--font-display:/);
	assert.match(tokens, /--font-brand: ui-rounded/);
	assert.match(tokens, /--font-mono:/);
	assert.match(tokens, /Noto Sans CJK SC/);
	assert.match(tokens, /Noto Serif CJK SC/);
	assert.match(tokens, /Apple Color Emoji/);
});

test('body applies the system sans stack and font smoothing', () => {
	const app = read('src/styles/app.css');
	assert.match(app, /font-family: var\(--font-sans\)/);
	assert.match(app, /text-rendering: optimizeLegibility/);
	assert.match(app, /-webkit-font-smoothing: antialiased/);
	assert.match(app, /-moz-osx-font-smoothing: grayscale/);
});

test('brand marks use rounded fonts while page headers keep display serif', () => {
	assert.match(read('src/layout/layout.css'), /\.app__logo-text \{[^}]*font-family: var\(--font-brand\)/);
	assert.match(read('src/components/components.css'), /\.page-header__title \{[^}]*font-family: var\(--font-display\)/);
	assert.match(read('src/pages/login.css'), /\.login-card__brand h1 \{[^}]*font-family: var\(--font-brand\)/);
	assert.doesNotMatch(read('src/pages/manga.css'), /font-family: var\(--font-display\)/);
	assert.doesNotMatch(read('src/pages/pages.css'), /font-family: var\(--font-display\)/);
	assert.match(read('src/pages/settings.css'), /font-family: var\(--font-mono\)/);
});
