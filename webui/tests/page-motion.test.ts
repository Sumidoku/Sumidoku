import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

function readSrc(path: string): string {
	return readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8').replace(/\r\n/g, '\n');
}

function withoutComments(css: string): string {
	return css.replace(/\/\*[\s\S]*?\*\//g, '');
}

function ruleBody(css: string, selector: string): string {
	const normalizedSelector = selector.replace(/\s+/g, ' ').trim();
	const escaped = normalizedSelector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = css.replace(/\s+/g, ' ').match(new RegExp(`(?:^|[}\\s])${escaped}\\s*\\{([^}]*)\\}`, 'm'));
	assert.ok(match, `missing rule ${selector}`);
	return match[1];
}

test('route changes swap in place without a page entrance or view transition', () => {
	const appCss = withoutComments(readSrc('styles/app.css'));
	const pageRule = appCss.match(/(?:^|[}\s])\.page\s*\{([^}]*)\}/m)?.[1] ?? '';
	assert.doesNotMatch(pageRule, /\banimation\s*:/);
	assert.doesNotMatch(appCss, /@keyframes\s+(page-in|vt-in|vt-out)\b/);
	assert.doesNotMatch(appCss, /\bview-transition-name\s*:/);

	// A stale sessionStorage entry makes React Router start a UA view transition on history
	// POP; every layer of the pseudo tree must be inert so even that swaps without a fade.
	const inertLayers = ruleBody(
		appCss,
		'::view-transition-group(*), ::view-transition-image-pair(*), ::view-transition-old(*), ::view-transition-new(*)'
	);
	assert.match(inertLayers, /\banimation:\s*none\s*!important\s*;/);
	assert.match(inertLayers, /\bmix-blend-mode:\s*normal\s*!important\s*;/);
	assert.match(ruleBody(appCss, '::view-transition-old(*)'), /\bdisplay:\s*none\s*!important\s*;/);

	for (const file of ['layout/MobileTabbar.tsx', 'layout/DesktopSidebar.tsx', 'components/CoverCard.tsx']) {
		assert.doesNotMatch(readSrc(file), /\bviewTransition\b/, `${file} must not opt into route view transitions`);
	}
});

test('covers paint as soon as loaded without a mount fade or zoom', () => {
	const css = withoutComments(readSrc('components/components.css'));
	const img = ruleBody(css, '.cover__img');
	assert.doesNotMatch(img, /\btransition\s*:/, 'mounting a cover must not transition opacity or transform');
	assert.doesNotMatch(img, /\btransform\s*:/, 'an unloaded cover must not start scaled up');
	// Keep the unloaded image hidden until onLoad so alt text never paints over the shimmer
	assert.match(img, /\bopacity:\s*0\s*;/);

	const loaded = ruleBody(css, '.cover__img--loaded');
	assert.match(loaded, /\bopacity:\s*1\s*;/);
	assert.doesNotMatch(loaded, /\btransform\s*:/);
	assert.doesNotMatch(loaded, /\btransition\s*:/);
});

test('cover zoom transition only runs on devices that can hover', () => {
	const css = withoutComments(readSrc('components/components.css'));
	const coverCardStart = css.indexOf('.cover-card {');
	assert.ok(coverCardStart >= 0);
	const coverCardCss = css.slice(coverCardStart);

	// Every cover-image transform transition belongs inside @media (hover: hover)
	const hoverBlocks = [...coverCardCss.matchAll(/@media\s*\(hover:\s*hover\)\s*\{([\s\S]*?)\n\}/g)].map((m) => m[1]);
	assert.ok(hoverBlocks.length > 0, 'cover hover rules must be gated on hover capability');
	const outsideHover = coverCardCss.replace(/@media\s*\(hover:\s*hover\)\s*\{[\s\S]*?\n\}/g, '');
	assert.doesNotMatch(outsideHover, /\.cover__img[^{]*\{[^}]*\btransition\s*:/);
	assert.doesNotMatch(outsideHover, /\.cover__img[^{]*\{[^}]*\btransform\s*:\s*scale\(/);
	assert.match(hoverBlocks.join('\n'), /\.cover-card:hover\s+\.cover__img--loaded\s*\{[^}]*transform:\s*scale\(1\.05\)/);

	const coverTsx = readSrc('components/Cover.tsx');
	assert.match(coverTsx, /cover--shimmer/);
	assert.match(coverTsx, /onError=\{\(\) => setFailedSrc/);
});
