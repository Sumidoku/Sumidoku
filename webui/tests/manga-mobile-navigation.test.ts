import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const css = readFileSync(new URL('../src/pages/manga.css', import.meta.url), 'utf8');

test('mobile detail back does not reuse the reader back position', () => {
	const mobile = css.match(/@media \(max-width: 767px\) \{([\s\S]*)\}\s*$/)?.[1];
	assert.ok(mobile);
	const back = mobile.match(/\.manga-hero__back\s*\{([^}]*)\}/);
	assert.ok(back);
	assert.match(back[1], /\bleft:\s*auto;/);
	assert.match(back[1], /\bright:\s*10px;/);
});

test('desktop detail back keeps its position', () => {
	const back = css.match(/\.manga-hero__back\s*\{([^}]*)\}/);
	assert.ok(back);
	assert.match(back[1], /\bleft:\s*10px;/);
	assert.doesNotMatch(back[1], /\bright:/);
});

test('mobile brand is not a home shortcut at the reader back coordinate', () => {
	const layout = readFileSync(new URL('../src/layout/AppLayout.tsx', import.meta.url), 'utf8');
	const topbar = layout.match(/<header className="app__topbar">([\s\S]*?)<\/header>/)?.[1];
	assert.ok(topbar);
	assert.doesNotMatch(topbar, /<NavLink/);
	assert.match(topbar, /<div className="app__logo app__logo--sm">/);
});

test('detail back rules outrank the shared icon button', () => {
	// manga.css is bundled before components.css, so an equal-specificity
	// .icon-btn would win and turn the back control into a dim square.
	const rules = css.replace(/\/\*[\s\S]*?\*\//g, '');
	const selectors = [...rules.matchAll(/([^{}]*\.manga-hero__back[^{}]*)\{/g)].map((m) => m[1].trim());
	assert.ok(selectors.length >= 3);
	for (const selector of selectors) {
		for (const part of selector.split(',')) {
			assert.match(part.trim(), /\.icon-btn\.manga-hero__back\b/, part.trim());
		}
	}
});
