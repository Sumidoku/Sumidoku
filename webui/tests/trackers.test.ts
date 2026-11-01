import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
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

test('tracker edit state updaters do not defer synthetic event reads', async () => {
	const source = (await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx?raw')).default;
	const updaters = [...source.matchAll(/set(?:Values|Edits)\(\s*\(?current\)?\s*=>[^\n]*/g)].map((match) => match[0]);
	assert.ok(updaters.length > 0);
	const deferredEventReads = updaters.filter((updater) => updater.includes('event.currentTarget'));
	assert.deepEqual(deferredEventReads, [], 'state updaters must capture input values before the event handler returns');
});

test('tracker edit merges fresh remote state with only explicit edits', async () => {
	const source = (await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx?raw')).default;
	assert.match(source, /mergeTrackEditValues\(initialState, edits\)/);
	const refreshed = { ...remoteState(), score: 9, lastReadChapter: 20, lastReadVolume: 4 };
	const merged = trackers.mergeTrackEditValues(refreshed, { score: '7' });
	assert.deepEqual(merged, { ...trackers.remoteToTrackEdit(refreshed), score: '7' });
	assert.deepEqual(trackers.buildTrackUpdate(refreshed, merged), { score: 7 });
	assert.deepEqual(trackers.buildTrackUpdate(refreshed, trackers.mergeTrackEditValues(refreshed, {})), {});
});

test('reverting a tracker edit does not override a later remote refresh', async () => {
	const source = (await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx?raw')).default;
	assert.match(source, /updateTrackEditValue\(initialState, current, field, value\)/);
	const initial = remoteState();
	const edited = trackers.updateTrackEditValue(initial, {}, 'score', '7');
	assert.deepEqual(edited, { score: '7' });
	const reverted = trackers.updateTrackEditValue(initial, edited, 'score', '8');
	assert.deepEqual(reverted, {});
	const refreshed = { ...initial, score: 9 };
	assert.deepEqual(trackers.buildTrackUpdate(refreshed, trackers.mergeTrackEditValues(refreshed, reverted)), {});
});

test('tracker add stays disabled while bindings are pending', async () => {
	const { createElement } = await import('react');
	const { renderToStaticMarkup } = await import('react-dom/server');
	const { QueryClient, QueryClientProvider } = await import('@tanstack/react-query');
	const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
	await i18n.changeLanguage('en');
	const { TrackerSection } = await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx');
	const markup = renderToStaticMarkup(
		createElement(QueryClientProvider, { client: new QueryClient() }, createElement(TrackerSection, { mangaId: 1, mangaTitle: 'Demo' }))
	);
	const addButton = [...markup.matchAll(/<button\b[^>]*>[\s\S]*?<\/button>/g)].find(([button]) => button.includes('Add binding'));
	assert.ok(addButton);
	assert.match(addButton[0], /\bdisabled=""/);
});

test('tracker save re-reads remote state and refreshes links on settled', async () => {
	const source = (await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx?raw')).default;
	assert.match(source, /onSettled: \(\) => \{/);
	assert.match(source, /void qc\.invalidateQueries\(\{ queryKey: trackerKeys\.state\(link\.provider, link\.trackId\) \}\);/);
	assert.doesNotMatch(source, /qc\.setQueryData<RemoteState>/);
});

test('tracker save retries from the last successfully saved binding', async () => {
	const source = (await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx?raw')).default;
	assert.match(source, /const \[currentLink, setCurrentLink\] = useState\(link\);/);
	assert.match(source, /setCurrentLink\(await putTrackerLink\(currentLink\.mangaId, plan\.link\)\)/);
	const initial = remoteState();
	const saved = link({ trackMode: 'volume', chapterOffset: -3 });
	const retry = trackers.buildTrackerSave(saved, 'auto', 0, initial, trackers.remoteToTrackEdit(initial));
	assert.deepEqual(retry.link, {
		provider: 'anilist',
		trackId: '123',
		title: 'Remote title',
		trackMode: 'auto',
		chapterOffset: 0
	});
	assert.deepEqual(retry.remoteUpdate, {});
});

test('tracker disconnect invalidates cached remote state', async () => {
	const source = (await vite.ssrLoadModule('/src/pages/settings/TrackersPanel.tsx?raw')).default;
	assert.match(source, /void qc\.invalidateQueries\(\{ queryKey: trackerKeys\.states \}\);/);
});

test('binding mutations invalidate manga and library projections', async () => {
	assert.equal(typeof trackers.invalidateTrackerLinks, 'function');
	const { QueryClient } = await import('@tanstack/react-query');
	const client = new QueryClient();
	const calls: unknown[] = [];
	client.invalidateQueries = ((filters: { queryKey: unknown }) => {
		calls.push(filters.queryKey);
		return Promise.resolve();
	}) as QueryClient['invalidateQueries'];
	trackers.invalidateTrackerLinks(client, 7);
	assert.deepEqual(calls, [trackers.trackerKeys.links, ['library'], ['manga', 7]]);
});

test('search results label series by medium and fall back to a generic series label', () => {
	assert.equal(trackers.trackerMediaLabelKey({ mediaType: 'manga', series: true }), 'trackers.mediaSeries.manga');
	assert.equal(trackers.trackerMediaLabelKey({ series: true }), 'trackers.mediaSeries.other');
	assert.equal(trackers.trackerMediaLabelKey({ mediaType: 'novel', series: false }), 'trackers.media.novel');
	assert.equal(trackers.trackerMediaLabelKey({ mediaType: 'manga' }), 'trackers.media.manga');
	assert.equal(trackers.trackerMediaLabelKey({}), null);
});

test('bindings gather per manga in page order', () => {
	const entry = (mangaId: number, provider: 'anilist' | 'bangumi') => ({ ...link(), mangaId, provider });
	const groups = trackers.groupBindings([entry(1, 'anilist'), entry(1, 'bangumi'), entry(2, 'bangumi')]);
	assert.deepEqual(
		groups.map((group) => group.map((item) => `${item.mangaId}:${item.provider}`)),
		[['1:anilist', '1:bangumi'], ['2:bangumi']]
	);
	assert.deepEqual(trackers.groupBindings([]), []);
});

test('the hero primary button keeps its fill on hover in both themes', () => {
	const css = readFileSync(new URL('../src/pages/manga.css', import.meta.url), 'utf8').replaceAll('\r\n', '\n');
	for (const prefix of ['', "[data-theme='light'] "]) {
		const rule = `${prefix}.manga-hero__actions .btn--primary:hover:not(:disabled) {\n\tbackground: var(--accent-solid-hover);`;
		assert.ok(css.includes(rule), `missing: ${rule}`);
	}
});

test('preferredTrackerProvider preselects a free default and falls back to the first free provider', () => {
	assert.equal(trackers.preferredTrackerProvider('bangumi', ['anilist', 'mal', 'bangumi']), 'bangumi');
	assert.equal(trackers.preferredTrackerProvider('bangumi', ['anilist', 'mal']), 'anilist');
	assert.equal(trackers.preferredTrackerProvider(null, ['mal', 'bangumi']), 'mal');
	assert.equal(trackers.preferredTrackerProvider(undefined, ['mal']), 'mal');
	assert.equal(trackers.preferredTrackerProvider('bangumi', []), 'anilist');
});

test('a rate limit is translated by its code while other failures keep the server text', async () => {
	const { default: i18n } = await vite.ssrLoadModule('/src/i18n/index.ts');
	await i18n.changeLanguage('zh-CN');
	const limited = new ApiError(429, 'tracker_rate_limited', 'bangumi rate limit reached; try again later');
	assert.equal(trackers.trackerErrorMessage(limited), '追踪器请求过于频繁，已被限流，请稍后再试');
	await i18n.changeLanguage('en');
	assert.equal(trackers.trackerErrorMessage(limited), 'The tracker is rate limiting requests. Try again in a while.');

	const failed = new ApiError(502, 'bad_gateway', 'anilist tracker: request returned 503 Service Unavailable');
	assert.equal(trackers.trackerErrorMessage(failed), failed.message);
});
