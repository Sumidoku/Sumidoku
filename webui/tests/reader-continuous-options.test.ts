import assert from 'node:assert/strict';
import test from 'node:test';
import { continuousImageStyle, continuousPageStyle, parseContinuousScale, readBoundedNumber } from '../src/pages/reader/continuousPrefs.ts';

test('continuous width keeps the existing strip width when padding is zero', () => {
	assert.deepEqual(continuousPageStyle('webtoon', 'width', 0, 0, true, null), {
		width: 'calc(100% - 0px)',
		marginTop: 0
	});
	assert.deepEqual(continuousPageStyle('continuous', 'width', 0, 0, true, null).width, 'calc(100% - 16px)');
});

test('continuous options set side padding, page gap and measured original width', () => {
	assert.deepEqual(continuousPageStyle('webtoon', 'width', 20, 15, false, { width: 800, height: 1200 }), {
		width: 'calc(60% - 0px)',
		marginTop: 15
	});
	assert.equal(continuousPageStyle('webtoon', 'original', 20, 0, true, { width: 800, height: 1200 }).width, '800px');
	assert.deepEqual(continuousImageStyle({ width: 800, height: 1200 }), { aspectRatio: '800 / 1200' });
	assert.deepEqual(continuousImageStyle(null), { aspectRatio: undefined });
});

test('stored continuous options accept only bounded values', () => {
	assert.equal(parseContinuousScale('original'), 'original');
	assert.equal(parseContinuousScale('screen'), null);
	assert.equal(readBoundedNumber('20', 40), 20);
	assert.equal(readBoundedNumber('45', 40), null);
	assert.equal(readBoundedNumber('1.5', 40), null);
	assert.equal(readBoundedNumber('', 40), null);
	assert.equal(readBoundedNumber(null, 40), null);
});
