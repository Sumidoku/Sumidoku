export type Dir = 'ltr' | 'rtl';

/**
 * In rtl the leading edge is the left side, so the left tap zone and ArrowLeft move
 * forward in reading order while the right side moves back; ltr is the mirror image.
 */
export function zoneMovesForward(dir: Dir, which: 'left' | 'right'): boolean {
	return dir === 'ltr' ? which === 'right' : which === 'left';
}

/**
 * Signed translateX for the paged text-column track. The track gets `direction: rtl`
 * under rtl, so the first column sits on the right and later columns overflow to the
 * left; moving forward then translates the track rightward (positive) instead of
 * leftward as under ltr.
 */
export function textColumnOffset(dir: Dir, col: number, width: number, gap: number): number {
	const distance = col * (width + gap);
	// + 0 normalizes -0 so the first column renders translateX(0px) under both directions
	return (dir === 'rtl' ? distance : -distance) + 0;
}
