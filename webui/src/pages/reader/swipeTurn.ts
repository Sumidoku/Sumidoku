/**
 * Swipe-to-turn decision for the paged reader. The drag itself is tracked and
 * rendered in ReaderPage; this module only decides what a release means. A
 * release turns the page when the drag crossed the turn distance in a
 * direction, or when it was a flick — fast enough with at least a small
 * travel, so a quick confident swipe never needs the full drag. Anything
 * short of that springs back to the current page.
 *
 * The turn distance is half the viewport capped at an absolute maximum: a
 * spread always fills the viewport, so in double-page layout half the viewport
 * is a whole page wide, and on a desktop window it grows far beyond any
 * natural drag. The cap keeps the required travel the same in every layout.
 */
export const SWIPE_TURN_FRACTION = 0.5;
export const SWIPE_TURN_MAX_PX = 180;
export const SWIPE_FLICK_PX_PER_MS = 0.5;
export const SWIPE_FLICK_MIN_PX = 24;

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
	if (fwdOffset >= threshold) return 'forward';
	if (fwdOffset <= -threshold) return 'backward';
	if (fwdVelocity >= SWIPE_FLICK_PX_PER_MS && fwdOffset >= SWIPE_FLICK_MIN_PX) return 'forward';
	if (fwdVelocity <= -SWIPE_FLICK_PX_PER_MS && fwdOffset <= -SWIPE_FLICK_MIN_PX) return 'backward';
	return null;
}
