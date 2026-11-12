/**
 * Swipe-to-turn decision for the paged reader. The drag itself is tracked and
 * rendered in ReaderPage; this module only decides what a release means. A
 * release turns the page when the drag travelled the turn distance in a
 * direction, or when it was a flick: fast with at least a small travel, so a
 * quick confident swipe never needs the full drag. Anything short of that
 * springs back to the current page.
 *
 * The turn distance scales with the viewport. One pixel travel does not read
 * the same everywhere: a fixed 40px was ~10% of a phone (what a thumb reads
 * as a swipe, in the band of the system back gesture) but under 4% of a
 * tablet, where a horizontal wobble during a scroll carried the page away.
 * A fraction of the width keeps the gesture proportional to what it moves —
 * and the drag is 1:1, so 8% of the width is 8% of the page's own travel.
 * The clamp keeps a tiny window from having no threshold at all and a
 * desktop window from needing a half-screen drag.
 */
export const SWIPE_TURN_FRACTION = 0.08;
export const SWIPE_TURN_MIN_PX = 32;
export const SWIPE_TURN_MAX_PX = 56;
export const SWIPE_FLICK_PX_PER_MS = 0.2;
export const SWIPE_FLICK_MIN_PX = 16;
/**
 * The release rescue also wants the gesture to be clearly horizontal. Without
 * the margin a diagonal reads as a swipe and the vertical scroll it was
 * mostly made of never happens.
 */
export const SWIPE_RESCUE_RATIO = 1.25;
/**
 * Release velocity is measured over the last ~100ms of the drag only: a
 * finger slows before it lifts, so an average of the whole drag under-reads
 * the flick and honest swipes spring back.
 */
export const SWIPE_VELOCITY_WINDOW_MS = 100;

/**
 * The slow-drag travel that turns a page at a given viewport width.
 */
export function swipeTurnThreshold(viewportWidth: number): number {
	return Math.min(Math.max(viewportWidth * SWIPE_TURN_FRACTION, SWIPE_TURN_MIN_PX), SWIPE_TURN_MAX_PX);
}

/**
 * Velocity over the window that ends at the release, in px/ms. A finger that
 * holds still sends no pointer moves, so the window slides past the last
 * sample: the still time counts as zero travel, and a flick followed by a
 * pause before the lift reads as slow. The trail is ordered by time and keeps
 * at least one sample from before the window when the drag is longer.
 */
export function releaseVelocity(trail: readonly { x: number; t: number }[], now: number): number {
	if (trail.length < 2) return 0;
	const from = now - SWIPE_VELOCITY_WINDOW_MS;
	// The finger's position when the window opened: the last sample at or
	// before it, or the drag's start when the whole drag is shorter
	let base = trail[0];
	for (const sample of trail) {
		if (sample.t > from) break;
		base = sample;
	}
	const span = now - Math.max(base.t, from);
	return span > 0 ? (trail[trail.length - 1].x - base.x) / span : 0;
}

export type SwipeDecision = 'forward' | 'backward' | null;

/**
 * offset and velocity are screen-space (negative x = moving left).
 * forwardSign maps them into reading direction (LTR: -1, RTL: +1), so a
 * positive forward offset means the drag went toward the next page.
 * threshold is swipeTurnThreshold(viewportWidth), measured once at the lift.
 */
export function decideSwipe(offset: number, velocity: number, forwardSign: 1 | -1, threshold: number): SwipeDecision {
	const fwdOffset = offset * forwardSign;
	const fwdVelocity = velocity * forwardSign;
	if (fwdOffset >= threshold) return 'forward';
	if (fwdOffset <= -threshold) return 'backward';
	// A fast flick turns on minimal travel even when the drag is short
	if (fwdVelocity >= SWIPE_FLICK_PX_PER_MS && fwdOffset >= SWIPE_FLICK_MIN_PX) return 'forward';
	if (fwdVelocity <= -SWIPE_FLICK_PX_PER_MS && fwdOffset <= -SWIPE_FLICK_MIN_PX) return 'backward';
	return null;
}

/**
 * The axis lock renders the drag and has to be decided early; the commit does
 * not have to agree with it. Once a gesture locked to y the page never
 * followed the finger, so a release that turns is a surprise — unless the
 * spread scrolled, in which case the gesture was a scroll and the browser
 * already used it. `scrolled` is measured by ReaderPage; this only answers
 * whether the release may still count as a horizontal turn.
 */
export function rescuesHorizontal(
	axis: 'x' | 'y' | null,
	totalDx: number,
	totalDy: number,
	scrolled: boolean,
	threshold: number,
): boolean {
	if (axis === 'x') return true;
	if (axis !== 'y' || scrolled) return false;
	const dx = Math.abs(totalDx);
	const dy = Math.abs(totalDy);
	return dx >= threshold && dx >= SWIPE_RESCUE_RATIO * dy;
}
