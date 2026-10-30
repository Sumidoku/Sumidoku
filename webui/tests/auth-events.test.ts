import assert from 'node:assert/strict';
import test from 'node:test';
import { authAllowsEvents } from '../src/lib/auth.ts';

test('SSE starts only with a usable authentication state', () => {
	assert.equal(authAllowsEvents(undefined), false);
	assert.equal(authAllowsEvents({ enabled: true, user: null }), false);
	assert.equal(authAllowsEvents({ enabled: false, user: null }), true);
	assert.equal(authAllowsEvents({ enabled: true, user: { id: 1, username: 'admin' } }), true);
});
