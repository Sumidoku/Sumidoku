/**
 * Swipe-to-turn decision for the paged reader. The drag itself is tracked and
 * rendered in ReaderPage; this module only decides what a release means. A
 * release turns the page when the drag crossed half the viewport in a
 * direction, or when it was a flick — fast enough with at least a small
 * travel, so a quick confident swipe never needs the full half. Anything
 * short of that springs back to the current page.
 */
export const SWIPE_TURN_FRACTION = 0.5;
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
	if (fwdOffset >= viewportWidth * SWIPE_TURN_FRACTION) return 'forward';
	if (fwdOffset <= -viewportWidth * SWIPE_TURN_FRACTION) return 'backward';
	if (fwdVelocity >= SWIPE_FLICK_PX_PER_MS && fwdOffset >= SWIPE_FLICK_MIN_PX) return 'forward';
	if (fwdVelocity <= -SWIPE_FLICK_PX_PER_MS && fwdOffset <= -SWIPE_FLICK_MIN_PX) return 'backward';
	return null;
}
