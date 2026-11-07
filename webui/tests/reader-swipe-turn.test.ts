// The swipe-to-turn decision is what makes mid-chapter drags feel honest:
// the release turns when the drag's projected landing — where the release
// velocity would carry it over the next 150ms — crosses the turn distance (a
// quarter of the viewport, capped so double-page spreads and desktop windows
// do not demand an impossible drag), or when a fast flick travels a short
// minimum; anything less springs back. These pins cover both reading
// directions, since screen signs flip with RTL.
import test from 'node:test';
import assert from 'node:assert/strict';

import { decideSwipe, releaseVelocity } from '../src/pages/reader/swipeTurn.ts';

test('crossing the turn distance turns the page in that direction (LTR)', () => {
	// LTR: forward is a leftward (negative) drag; 1000px viewport is past
	// the cap, so the threshold is the 96px cap
	assert.equal(decideSwipe(-96, 0, 1000, -1), 'forward');
	assert.equal(decideSwipe(96, 0, 1000, -1), 'backward');
});

test('crossing the turn distance turns the page in that direction (RTL)', () => {
	// RTL: forward is a rightward (positive) drag
	assert.equal(decideSwipe(96, 0, 1000, 1), 'forward');
	assert.equal(decideSwipe(-96, 0, 1000, 1), 'backward');
});

test('just short of the turn distance springs back', () => {
	// Wide viewport: the cap is the threshold, so 95px stays short
	assert.equal(decideSwipe(-95, 0, 1000, -1), null);
	assert.equal(decideSwipe(95, 0, 1000, 1), null);
});

test('the turn distance is capped on wide viewports (double-page, desktop)', () => {
	// A 1400px spread viewport would demand a 350px drag without the cap
	assert.equal(decideSwipe(-96, 0, 1400, -1), 'forward');
	assert.equal(decideSwipe(96, 0, 1400, 1), 'forward');
});

test('a narrow viewport keeps the quarter as its threshold', () => {
	// 300px viewport: a quarter (75px) is below the cap and stays the threshold
	assert.equal(decideSwipe(-75, 0, 300, -1), 'forward');
	assert.equal(decideSwipe(-74, 0, 300, -1), null);
});

test('a drag released with forward momentum turns on its projected landing', () => {
	// 60px is short of the 96px distance, but at 0.4px/ms the projection over
	// 150ms lands at 120px — a confident swipe never needs the full drag
	assert.equal(decideSwipe(-60, -0.4, 1000, -1), 'forward');
	// The same travel released dead springs back
	assert.equal(decideSwipe(-60, 0, 1000, -1), null);
});

test('a release already moving back cancels the turn its travel earned', () => {
	// 120px of forward travel, but the finger reversed before lifting: the
	// projection falls short and the page springs back
	assert.equal(decideSwipe(-120, 0.5, 1000, -1), null);
});

test('a fast flick turns with little travel, a slow drag with the same travel does not', () => {
	// 16px at 0.25px/ms: projection (53.5px) falls short of the distance, the
	// flick gate carries it
	assert.equal(decideSwipe(-16, -0.25, 1000, -1), 'forward');
	// Same travel one notch slower turns nothing
	assert.equal(decideSwipe(-16, -0.15, 1000, -1), null);
});

test('a flick still needs minimum travel, and only counts in its own direction', () => {
	assert.equal(decideSwipe(-15, -0.25, 1000, -1), null);
	// Fast but backward: not a forward turn
	assert.equal(decideSwipe(30, 0.6, 1000, -1), 'backward');
	assert.equal(decideSwipe(-30, 0.6, 1000, -1), null);
});

test('degenerate inputs never turn', () => {
	assert.equal(decideSwipe(0, 0, 1000, -1), null);
	assert.equal(decideSwipe(-500, 0, 0, -1), null);
	assert.equal(decideSwipe(-500, 0, -100, -1), null);
});

function steadyDrag(px: number, ms: number) {
	// One sample every 10ms along a steady drag from 0 to px
	return Array.from({ length: ms / 10 + 1 }, (_, i) => ({ x: (px * i * 10) / ms, t: i * 10 }));
}

test('a flick lifted at once reads at its own speed', () => {
	assert.equal(releaseVelocity(steadyDrag(-100, 100), 100), -1);
	// A drag shorter than the window is measured from its start
	assert.equal(releaseVelocity(steadyDrag(-30, 30), 30), -1);
});

test('stopping before the lift drains the release velocity', () => {
	// No pointer moves arrive while the finger holds still, so the window
	// slides past the last sample and the still time counts as no travel
	const trail = steadyDrag(-60, 100);
	assert.equal(releaseVelocity(trail, 150), -0.3);
	assert.equal(releaseVelocity(trail, 300), 0);
	// The drag kept its 60px, but held still it is no longer a flick or a
	// projected landing past the turn distance
	assert.equal(decideSwipe(-60, releaseVelocity(trail, 300), 1000, -1), null);
	assert.equal(decideSwipe(-60, releaseVelocity(trail, 100), 1000, -1), 'forward');
});

test('a trail too short to measure has no velocity', () => {
	assert.equal(releaseVelocity([], 0), 0);
	assert.equal(releaseVelocity([{ x: 5, t: 0 }], 50), 0);
});

// Wiring pins: the swipe decision only matters while the release routes
// through the same turn() the tap zones use, and the text column strip is the
// follow target on text pages
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const textColumns = readFileSync(new URL('../src/pages/reader/TextColumns.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

test('a swipe release turns through the shared turn path', () => {
	assert.match(source, /decideSwipe\(totalDx, velocity, root\.clientWidth, forwardSignRef\.current\)/);
	assert.match(source, /turnRef\.current\(decision === 'forward'\);/);
});

test('the release velocity comes from a trailing window, not a drag-long average', () => {
	// A finger slows before it lifts, so an EMA of the whole drag under-reads
	// the release and honest flicks spring back; the velocity must be the
	// last ~100ms of movement only
	assert.match(source, /SWIPE_VELOCITY_WINDOW_MS/);
	assert.match(source, /while \(trail\.length > 2 && now - trail\[0\]\.t > SWIPE_VELOCITY_WINDOW_MS\) trail\.shift\(\);/);
	assert.doesNotMatch(source, /0\.7 \* velocity/);
	// Read at the lift: the last move's speed is stale after a pause
	assert.match(source, /const velocity = releaseVelocity\(trail, performance\.now\(\)\);/);
});

test('the gesture effect re-attaches once pages mount the paged container', () => {
	// .reader__paged only renders after the pages query resolves, so an
	// effect keyed on mode/navReady alone runs while the ref is still null
	// on first entry and its listeners never attach until the mode toggles
	const start = source.indexOf('// Paged pointer gestures.');
	assert.ok(start >= 0);
	const end = source.indexOf('}, [', start);
	assert.ok(end > start);
	assert.ok(
		source.slice(start, end + 32).includes('}, [mode, pages, navReady]'),
		'the paged gesture effect must re-run when pages mounts the container',
	);
});

test('the column strip follows the finger through its own variable and transition cut', () => {
	assert.match(source, /cols\.classList\.add\('reader__swiping'\);/);
	assert.match(textColumns, /var\(--text-swipe, 0px\)/);
	assert.match(css, /\.reader__paged\[data-anim='true'\] \.reader-text--cols\.reader__swiping \{\n\ttransition: none;/);
});

test('a pull that owns the drag keeps the swipe out of the shared variable', () => {
	assert.match(source, /if \(ctl\.feedTouch\(dx \* forwardSignRef\.current, true\)\) \{\n\t\t\t\t\tpullOwned = true;/);
});

test('a second finger abandons the drag before the primary-pointer filter drops it', () => {
	// A second touch arrives with isPrimary false, so a dragId check placed
	// after the isPrimary filter never runs and a pinch at scale 1 keeps
	// swiping the first finger's travel into a page turn
	const handler = source.slice(source.indexOf('const onPointerDown = (e: PointerEvent) => {'));
	const abandon = handler.indexOf('if (dragId !== null) {');
	const primaryFilter = handler.indexOf('if (!e.isPrimary || zoomedRef.current) return;');
	assert.ok(abandon >= 0 && primaryFilter >= 0);
	assert.ok(abandon < primaryFilter, 'the pinch abandon must run before non-primary pointers are filtered out');
});
