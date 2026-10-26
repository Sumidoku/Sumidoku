import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test, { after, before } from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import type { RemoteState, TrackerLink } from '../src/api/types.ts';
import type { TrackEditValues } from '../src/api/trackers.ts';

let vite: ViteDevServer;
let trackers: typeof import('../src/api/trackers.ts');
let ApiError: typeof import('../src/api/client.ts').ApiError;

before(async () => {
	vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	trackers = await vite.ssrLoadModule('/src/api/trackers.ts');
	({ ApiError } = await vite.ssrLoadModule('/src/api/client.ts'));
});

after(() => vite.close());

function link(overrides: Partial<TrackerLink> = {}): TrackerLink {
	return {
		mangaId: 7,
		provider: 'anilist',
		trackId: '123',
		title: 'Remote title',
		trackMode: 'auto',
		chapterOffset: 0,
		createdAt: 1,
		updatedAt: 2,
		...overrides
	};
}

function remoteState(): RemoteState {
	return {
		score: 8,
		status: 'reading',
		lastReadChapter: 12,
		lastReadVolume: 2,
		totalChapters: 100
	};
}

test('sparse track updates contain only edited remote fields', () => {
	const initial = remoteState();
	const values: TrackEditValues = {
		...trackers.remoteToTrackEdit(initial),
		status: 'completed',
		lastReadChapter: '12.5'
	};
	assert.deepEqual(trackers.buildTrackUpdate(initial, values), {
		status: 'completed',
		lastReadChapter: 12.5
	});
});

test('binding-only save plans never produce a remote update', () => {
	const initial = remoteState();
	const values = trackers.remoteToTrackEdit(initial);
	const plan = trackers.buildTrackerSave(link(), 'volume', -3, initial, values);
	assert.deepEqual(plan.link, {
		provider: 'anilist',
		trackId: '123',
		title: 'Remote title',
		trackMode: 'volume',
		chapterOffset: -3
	});
	assert.deepEqual(plan.remoteUpdate, {});
	assert.equal(trackers.hasTrackUpdate(plan.remoteUpdate), false);

	const unchanged = trackers.buildTrackerSave(link(), 'auto', 0, initial, values);
	assert.equal(unchanged.link, undefined);
	assert.deepEqual(unchanged.remoteUpdate, {});
});

test('equivalent numeric edits and null remote values stay sparse', () => {
	const initial = remoteState();
	const values = { ...trackers.remoteToTrackEdit(initial), score: '8.0' };
	assert.deepEqual(trackers.buildTrackUpdate(initial, values), {});
	assert.deepEqual(trackers.buildTrackUpdate({}, trackers.remoteToTrackEdit({})), {});
});

test('available providers exclude every existing binding', () => {
	const links = [link(), link({ provider: 'bangumi', trackId: '456' })];
	assert.deepEqual(trackers.availableTrackerProviders(links), ['mal', 'mangabaka']);
});

test('OAuth return cleanup retains section and unrelated query parameters', () => {
	assert.deepEqual(trackers.readOAuthReturn('section=trackers&oauth=error&message=bad+token&keep=1'), {
		status: 'error',
		message: 'bad token',
		cleanedSearch: 'section=trackers&keep=1'
	});
	assert.deepEqual(trackers.readOAuthReturn('oauth=success&section=trackers'), {
		status: 'success',
		message: null,
		cleanedSearch: 'section=trackers'
	});
	assert.equal(trackers.readOAuthReturn('section=trackers'), null);
	assert.equal(trackers.readOAuthReturn('section=trackers&oauth=other'), null);
});

test('only HTTP 409 tracker_auth requests account reconnection', () => {
	assert.equal(trackers.isTrackerAuthError(new ApiError(409, 'tracker_auth', 'connect')), true);
	assert.equal(trackers.isTrackerAuthError(new ApiError(401, 'tracker_auth', 'login')), false);
	assert.equal(trackers.isTrackerAuthError(new ApiError(409, 'bad_request', 'other')), false);
	assert.equal(trackers.isTrackerAuthError(new Error('tracker_auth')), false);
});
