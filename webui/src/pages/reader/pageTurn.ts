export type PageTurnStyle = 'slide' | 'fade';

/**
 * How long the outgoing page stays on top while the target page is still
 * loading. Long enough to cover a normal page load, short enough that a
 * failed image does not look like a stuck gesture.
 */
export const TURN_HOLD_MS = 500;

/**
 * A crossfade holds for less: the target page paints underneath from the
 * first frame, so the hold only has to hide the seam.
 */
export const TURN_HOLD_FADE_MS = 250;

/**
 * Horizontal travel that completes a crossfade while dragging. A crossfade
 * has no position to drag to, so the drag reads as a progress value instead:
 * this is the travel at which the outgoing page is fully gone.
 */
export const FADE_TRAVEL_PX = 160;

export function parsePageTurn(value: string | null): PageTurnStyle | null {
	if (value === 'slide' || value === 'fade') return value;
	// Stored 'auto' predates the split: landscape windows used to crossfade
	// and the rest slid. Sliding is the general case and what a drag shows.
	if (value === 'auto') return 'slide';
	return null;
}

export function crossfadeEnabled(style: PageTurnStyle, animated: boolean, split: boolean): boolean {
	return animated && !split && style === 'fade';
}

export function shouldRevealTurn(shownPage: number, targetPage: number, ready: boolean, split: boolean): boolean {
	return shownPage === targetPage || ready || split;
}

/**
 * Reversible crossfade progress for a drag toward `direction`: 0..capacity.
 * `travel` is forward-signed (positive = toward the next page). `capacity`
 * caps the fade while the target page is not loaded yet, so a slow page
 * never leaves a blank spread behind a finger.
 */
export function fadeProgress(travel: number, direction: 'next' | 'prev' | null, capacity: number): number {
	if (direction === null) return 0;
	const toward = direction === 'next' ? travel : -travel;
	return Math.min(Math.max(toward / FADE_TRAVEL_PX, 0), capacity);
}
