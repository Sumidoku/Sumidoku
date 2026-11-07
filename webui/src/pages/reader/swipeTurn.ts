/**
 * Swipe-to-turn decision for the paged reader. The drag itself is tracked and
 * rendered in ReaderPage; this module only decides what a release means. A
 * release turns the page when the drag's projected landing — where its
 * release velocity would carry it over the next 150ms — crosses the turn
 * distance in a direction, or when it was a flick: fast with at least a
 * small travel, so a quick confident swipe never needs the full drag.
 * Anything short of that springs back to the current page.
 *
 * The turn distance is a quarter of the viewport capped at an absolute
 * maximum: a spread always fills the viewport, so in double-page layout a
 * quarter of the viewport is half a page wide, and on a desktop window it
 * grows far beyond any natural drag. The cap keeps the required travel the
 * same in every layout.
 */
export const SWIPE_TURN_FRACTION = 0.25;
export const SWIPE_TURN_MAX_PX = 96;
export const SWIPE_PROJECTION_MS = 150;
export const SWIPE_FLICK_PX_PER_MS = 0.2;
export const SWIPE_FLICK_MIN_PX = 16;
/**
 * Release velocity is measured over the last ~100ms of the drag only: a
 * finger slows before it lifts, so an average of the whole drag under-reads
 * the flick and honest swipes spring back.
 */
export const SWIPE_VELOCITY_WINDOW_MS = 100;

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
 */
export function decideSwipe(offset: number, velocity: number, viewportWidth: number, forwardSign: 1 | -1): SwipeDecision {
	if (viewportWidth <= 0) return null;
	const fwdOffset = offset * forwardSign;
	const fwdVelocity = velocity * forwardSign;
	const threshold = Math.min(viewportWidth * SWIPE_TURN_FRACTION, SWIPE_TURN_MAX_PX);
	// A decisive swipe turns on where it would land, not on how far it was
	// dragged; a release already moving back cancels the travel it earned
	const projected = fwdOffset + fwdVelocity * SWIPE_PROJECTION_MS;
	if (projected >= threshold) return 'forward';
	if (projected <= -threshold) return 'backward';
	// A fast flick turns on minimal travel even when its projection falls short
	if (fwdVelocity >= SWIPE_FLICK_PX_PER_MS && fwdOffset >= SWIPE_FLICK_MIN_PX) return 'forward';
	if (fwdVelocity <= -SWIPE_FLICK_PX_PER_MS && fwdOffset <= -SWIPE_FLICK_MIN_PX) return 'backward';
	return null;
}
