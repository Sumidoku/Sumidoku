import assert from 'node:assert/strict';
import test from 'node:test';
import { editedMetadataValue } from '../src/lib/metadataValues.ts';

test('an edited list splits on commas and drops empty items', () => {
	assert.deepEqual(editedMetadataValue(['A'], ' B, ,C '), ['B', 'C']);
});

test('a cleared input clears the field instead of keeping the matched value', () => {
	assert.deepEqual(editedMetadataValue(['A'], ''), []);
	assert.equal(editedMetadataValue('Matched', ''), null);
	assert.equal(editedMetadataValue('Matched', '   '), null);
});

test('edited text is sent as typed', () => {
	assert.equal(editedMetadataValue('Matched', ' Edited '), ' Edited ');
});
