import assert from 'node:assert/strict';
import test from 'node:test';
import { compareVersions } from '../src/lib/version.ts';

test('v 前缀不影响比较', () => {
	assert.equal(compareVersions('v0.17.1', '0.17.1'), 0);
	assert.equal(compareVersions('v1.2.3', '1.2.3'), 0);
});

test('数字段按数值比，不按字典序', () => {
	assert.ok(compareVersions('0.17.1', '0.9.0') > 0);
	assert.ok(compareVersions('v0.9.0', 'v0.17.1') < 0);
	assert.ok(compareVersions('1.10.0', '1.9.9') > 0);
});

test('等号：同号、补零同号', () => {
	assert.equal(compareVersions('1.2.0', '1.2.0'), 0);
	assert.equal(compareVersions('1.2', '1.2.0'), 0);
	assert.equal(compareVersions('v1.2', '1.2.0'), 0);
});

test('预发布排在同号正式版之前', () => {
	assert.ok(compareVersions('1.0.0-beta', '1.0.0') < 0);
	assert.ok(compareVersions('1.0.0', '1.0.0-beta') > 0);
	assert.ok(compareVersions('1.0.0-beta', '1.0.1') < 0);
	assert.ok(compareVersions('1.0.0-alpha', '1.0.0-beta') < 0);
	assert.equal(compareVersions('1.0.0-beta', 'v1.0.0-beta'), 0);
});

test('乱序段：逐段比，首个不同段定胜负', () => {
	assert.ok(compareVersions('0.18.0', '0.17.9') > 0);
	assert.ok(compareVersions('2.0.0', '1.99.99') > 0);
	assert.ok(compareVersions('0.17.1', '0.17.2') < 0);
});

test('不可解析返回 null', () => {
	assert.equal(compareVersions('dev', '1.0.0'), null);
	assert.equal(compareVersions('1.0.0', ''), null);
});
