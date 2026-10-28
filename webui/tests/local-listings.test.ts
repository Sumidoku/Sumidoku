import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import i18n from 'i18next';
import sourceHomeZh from '../src/i18n/locales/zh-CN/sourceHome.json' with { type: 'json' };
import { homeSectionTitle, localListingLabel } from '../src/lib/localListings.ts';

await i18n.init({ lng: 'zh-CN', resources: { 'zh-CN': { translation: sourceHomeZh } } });
const t = i18n.t.bind(i18n);

test('local home titles translate by listing id, not by the English label', () => {
	assert.equal(homeSectionTitle('Latest updates', 'latest-updates', true, t), '最近更新');
	// A reworded server label must keep its translation
	assert.equal(homeSectionTitle('Recently added', 'latest-added', true, t), '最近添加');
	assert.equal(homeSectionTitle('Recently read', 'recently-read', true, t), '最近阅读');
});

test('non-local and unknown sections keep the source title', () => {
	assert.equal(homeSectionTitle('Latest updates', 'latest-updates', false, t), 'Latest updates');
	assert.equal(homeSectionTitle('Popular', null, true, t), 'Popular');
	assert.equal(homeSectionTitle('Popular', 'popular', true, t), 'Popular');
	assert.equal(localListingLabel('popular', 'Popular', t), 'Popular');
});

test('home and browse pages share the id-keyed local listing labels', () => {
	const home = readFileSync(new URL('../src/pages/SourceHomePage.tsx', import.meta.url), 'utf8');
	const browse = readFileSync(new URL('../src/pages/SourceBrowsePage.tsx', import.meta.url), 'utf8');
	assert.match(home, /homeSectionTitle\(component\.title, listing\?\.id \?\? null, isLocal, t\)/);
	assert.match(browse, /localListingLabel\(l\.id, l\.name, t\)/);
	for (const source of [home, browse]) {
		assert.doesNotMatch(source, /case 'Latest updates'/);
		assert.doesNotMatch(source, /startsWith\('sourceHome\.'\)/);
	}
});
