// The swipe-to-turn decision is what makes mid-chapter drags feel honest:
// the release turns when the finger travelled the turn distance in a
// direction, or when a fast flick carries it even with a short travel;
// anything less springs back. The turn distance follows the viewport width —
// 8% of it, clamped — because the drag is 1:1 and the same pixel travel reads
// as a very different share of the page on a phone and on a tablet. These
// pins cover both reading directions, since screen signs flip with RTL.
import test from 'node:test';
import assert from 'node:assert/strict';

import {
	decideSwipe,
	releaseVelocity,
	rescuesHorizontal,
	swipeTurnThreshold,
	SWIPE_TURN_MAX_PX,
	SWIPE_TURN_MIN_PX,
} from '../src/pages/reader/swipeTurn.ts';

// A phone viewport and a desktop one, as the release would measure them
const PHONE = swipeTurnThreshold(390);
const DESKTOP = swipeTurnThreshold(1440);

function near(actual: number, expected: number): boolean {
	return Math.abs(actual - expected) < 1e-9;
}

test('the turn distance follows the viewport width inside the clamp', () => {
	// 8% of a phone is under the floor, so small screens share one distance
	assert.equal(swipeTurnThreshold(320), SWIPE_TURN_MIN_PX);
	assert.ok(near(swipeTurnThreshold(390), SWIPE_TURN_MIN_PX));
	assert.ok(near(swipeTurnThreshold(400), SWIPE_TURN_MIN_PX) || swipeTurnThreshold(400) === SWIPE_TURN_MIN_PX);
	// In between it scales with the width
	assert.ok(near(swipeTurnThreshold(600), 48));
	// Wide surfaces stop at the ceiling instead of asking for a screen drag
	assert.equal(swipeTurnThreshold(768), SWIPE_TURN_MAX_PX);
	assert.equal(swipeTurnThreshold(1024), SWIPE_TURN_MAX_PX);
	assert.equal(swipeTurnThreshold(2400), SWIPE_TURN_MAX_PX);
	// Monotonic: a wider view never needs a shorter drag
	let previous = 0;
	for (const width of [280, 320, 390, 480, 600, 700, 768, 1024, 1440, 2560]) {
		const next = swipeTurnThreshold(width);
		assert.ok(next >= previous);
		assert.ok(next >= SWIPE_TURN_MIN_PX && next <= SWIPE_TURN_MAX_PX);
		previous = next;
	}
	assert.equal(PHONE, 32);
	assert.equal(DESKTOP, 56);
});

test('the turn distance is the same in both directions (LTR)', () => {
	// LTR: forward is a leftward (negative) drag
	assert.equal(decideSwipe(-PHONE, 0, -1, PHONE), 'forward');
	assert.equal(decideSwipe(PHONE, 0, -1, PHONE), 'backward');
	assert.equal(decideSwipe(-(PHONE - 1), 0, -1, PHONE), null);
});

test('the turn distance mirrors for RTL', () => {
	// RTL: forward is a rightward (positive) drag
	assert.equal(decideSwipe(PHONE, 0, 1, PHONE), 'forward');
	assert.equal(decideSwipe(-PHONE, 0, 1, PHONE), 'backward');
	assert.equal(decideSwipe(PHONE - 1, 0, 1, PHONE), null);
});

test('a tablet needs more travel than a phone for the same slow drag', () => {
	// The drag is 1:1, so the threshold is a share of the page's own travel
	assert.equal(decideSwipe(-40, 0, -1, PHONE), 'forward');
	assert.equal(decideSwipe(-40, 0, -1, DESKTOP), null);
	assert.equal(decideSwipe(-DESKTOP, 0, -1, DESKTOP), 'forward');
});

test('a fast flick turns with little travel, a slow drag with the same travel does not', () => {
	// 16px at 0.25px/ms: the flick gate carries it on every screen size
	assert.equal(decideSwipe(-16, -0.25, -1, DESKTOP), 'forward');
	// Same travel one notch slower turns nothing
	assert.equal(decideSwipe(-16, -0.15, -1, DESKTOP), null);
});

test('a flick still needs minimum travel, and only counts in its own direction', () => {
	assert.equal(decideSwipe(-15, -0.25, -1, PHONE), null);
	// Fast but backward: not a forward turn
	assert.equal(decideSwipe(30, 0.6, -1, PHONE), 'backward');
	assert.equal(decideSwipe(-30, 0.6, -1, PHONE), null);
});

test('a release that travelled past the distance turns regardless of its speed', () => {
	// A slow long drag is a deliberate swipe: no projection is needed
	assert.equal(decideSwipe(-PHONE, 0, -1, PHONE), 'forward');
	assert.equal(decideSwipe(-120, 0, -1, PHONE), 'forward');
});

test('degenerate inputs never turn', () => {
	assert.equal(decideSwipe(0, 0, -1, PHONE), null);
	assert.equal(decideSwipe(PHONE - 1, 0, -1, PHONE), null);
	assert.equal(decideSwipe(-(PHONE - 1), 0, 1, PHONE), null);
});

test('a release can rescue a gesture the axis lock had filed as vertical', () => {
	// Locked horizontal already: nothing to rescue
	assert.equal(rescuesHorizontal('x', 60, 40, false, PHONE), true);
	assert.equal(rescuesHorizontal('x', 5, 40, false, PHONE), true);
	// Locked vertical with no scroll: only a clearly horizontal travel counts
	assert.equal(rescuesHorizontal('y', 41, 30, false, PHONE), true);
	assert.equal(rescuesHorizontal('y', 40, 40, false, PHONE), false);
	assert.equal(rescuesHorizontal('y', 31, 10, false, PHONE), false);
	// The rescue follows the same viewport threshold as a normal turn
	assert.equal(rescuesHorizontal('y', 41, 10, false, DESKTOP), false);
	assert.equal(rescuesHorizontal('y', 57, 10, false, DESKTOP), true);
	// The spread scrolled: the browser already used the gesture
	assert.equal(rescuesHorizontal('y', 100, 10, true, PHONE), false);
	// No axis yet (the release came before the lock distance)
	assert.equal(rescuesHorizontal(null, 100, 10, false, PHONE), false);
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
	// The drag kept its 60px — past the phone turn distance — but held still it
	// is no longer a flick
	assert.equal(decideSwipe(-60, releaseVelocity(trail, 300), -1, PHONE), 'forward');
	assert.equal(decideSwipe(-16, releaseVelocity(trail, 300), -1, PHONE), null);
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
	// The threshold is measured at the lift against the container in use
	assert.match(source, /const threshold = swipeTurnThreshold\(root\.clientWidth\);/);
	assert.match(source, /decideSwipe\(totalDx, velocity, forwardSignRef\.current, threshold\)/);
	assert.match(source, /turnRef\.current\(decision === 'forward'\);/);
});

test('the axis lock no longer decides alone: the release rescues a vertical lock', () => {
	// The commit reads the rescue, and the rescue needs the scroll measured
	// from the spread the gesture started on
	assert.match(source, /rescuesHorizontal\(axis, totalDx, totalDy, scrolled, threshold\)/);
	assert.match(source, /const horizontal = !abandon && rescuesHorizontal\(/);
	assert.match(source, /\.closest\('\.reader__spread, \.reader-text--cols'\)/);
	assert.match(source, /Math\.abs\(start\.el\.scrollTop - start\.top\) > 2/);
});

test('a jittery tap still reaches a tap zone, a real drag never does', () => {
	// The click is judged by how far the pointer got from where it went down,
	// not by the length of the path travelled: a thumb that rolls while
	// tapping ends near where it started and must still click, while a swipe
	// that left the slop and came back must not (that is how a swipe cancels)
	assert.match(source, /const TAP_SLOP_PX = 10;/);
	assert.match(source, /const travel = Math\.hypot\(totalDx, totalDy\);/);
	assert.match(source, /if \(travel > maxTravel\) maxTravel = travel;/);
	assert.match(source, /if \(maxTravel > TAP_SLOP_PX \|\| had\) suppressClickRef\.current = true;/);
	assert.doesNotMatch(source, /moved \+= Math\.abs\(dx\) \+ Math\.abs\(dy\)/);
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
