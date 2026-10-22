import assert from 'node:assert/strict';
import test from 'node:test';
import i18n from 'i18next';
import formatZh from '../src/i18n/locales/zh-CN/format.json' with { type: 'json' };
import { chapterLabel, formatRangeBound } from '../src/lib/format.ts';

// The app registers every catalog through Vite's glob in src/i18n; under node the test
// pins just the zh-CN format catalog chapterLabel renders through
await i18n.init({
	resources: { 'zh-CN': { translation: formatZh } },
	lng: 'zh-CN',
	interpolation: { escapeValue: false }
});

const base = { volume_number: null, chapter_number: null, title: null };

test('卷 + 章 + 标题', () => {
	assert.equal(
		chapterLabel({ ...base, volume_number: 3, chapter_number: 5, title: '标题' }),
		'第 3 卷 · 第 5 话 · 标题'
	);
});

test('卷 + 章，无标题', () => {
	assert.equal(
		chapterLabel({ ...base, volume_number: 3, chapter_number: 5 }),
		'第 3 卷 · 第 5 话'
	);
});

test('仅卷号：卷合集条目不再落到未命名章节', () => {
	assert.equal(chapterLabel({ ...base, volume_number: 3 }), '第 3 卷');
});

test('卷 + 标题，无章节号', () => {
	assert.equal(
		chapterLabel({ ...base, volume_number: 3, title: '标题' }),
		'第 3 卷 · 标题'
	);
});

test('仅章节号 / 章节号 + 标题：与旧行为一致', () => {
	assert.equal(chapterLabel({ ...base, chapter_number: 5 }), '第 5 话');
	assert.equal(
		chapterLabel({ ...base, chapter_number: 5, title: '标题' }),
		'第 5 话 · 标题'
	);
});

test('仅标题 / 全空：与旧行为一致', () => {
	assert.equal(chapterLabel({ ...base, title: '标题' }), '标题');
	assert.equal(chapterLabel(base), '未命名章节');
});

test('空白标题按无标题处理', () => {
	assert.equal(chapterLabel({ ...base, volume_number: 3, title: '   ' }), '第 3 卷');
	assert.equal(chapterLabel({ ...base, chapter_number: 5, title: '' }), '第 5 话');
});

test('range bounds handle undefined, null and zero without placeholder text', () => {
	assert.equal(formatRangeBound(undefined), '');
	assert.equal(formatRangeBound(null), '');
	assert.equal(formatRangeBound(0), '0');
	assert.equal(formatRangeBound(1.5), '1.5');
});
