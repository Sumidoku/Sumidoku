import type { Viewer } from '../../api/types';

export type Dir = 'ltr' | 'rtl';

/**
 * Horizontal reading direction carried by manga metadata (Aidoku viewer, filled from the
 * source or from local EPUB spine / ComicInfo / series.json). Vertical and Webtoon say
 * nothing about horizontal direction, so they map to no preference.
 */
export function dirFromViewer(viewer: Viewer | null | undefined): Dir | null {
	if (viewer === 'RightToLeft') return 'rtl';
	if (viewer === 'LeftToRight') return 'ltr';
	return null;
}

/**
 * Effective direction: an explicit per-manga override beats the manga's metadata,
 * which beats the stored global default.
 */
export function resolveDir(override: Dir | null, meta: Dir | null, fallback: Dir): Dir {
	return override ?? meta ?? fallback;
}

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
