import type { Dir } from './readerPrefs';
import type { PageDims } from './spreads';

// Wide-page splitting (paged mode, single layout): a landscape page becomes two
// half-pages in reading order, each cropping one side of the same image, after
// Tachiyomi/Mihon's "split double pages". The split is a navigation concept
// only — progress, the slider and restore stay in real page indexes, so a
// chapter never reports fractional or inflated page counts.

// The side each half shows, in reading order: ltr reads the left half first
export type SplitSide = 'left' | 'right';

export function splitSides(dir: Dir): readonly [SplitSide, SplitSide] {
	return dir === 'rtl' ? ['right', 'left'] : ['left', 'right'];
}

// Same wide-page rule as the spread builder: strictly wider than tall. Pages
// without dimensions (unmeasured remote pages) are not split until recordMeasured
// fills them in — the halves then appear without moving the current page
export function isSplittable(dims: PageDims | null | undefined): dims is PageDims {
	return dims != null && dims.width > dims.height;
}

export interface SplitPos {
	page: number;
	// Reading-order half: 0 = first, 1 = second. Meaningless on unsplit pages
	half: 0 | 1;
}

// Next position when turning with split navigation; null at the chapter
// boundary (the caller switches chapters). Entering a page always lands on its
// first half, backward entry on the second half
export function turnSplit(
	pos: SplitPos,
	pageCount: number,
	landscape: (page: number) => boolean,
	forward: boolean
): SplitPos | null {
	if (forward) {
		if (landscape(pos.page) && pos.half === 0) return { page: pos.page, half: 1 };
		if (pos.page >= pageCount - 1) return null;
		return { page: pos.page + 1, half: 0 };
	}
	if (landscape(pos.page) && pos.half === 1) return { page: pos.page, half: 0 };
	if (pos.page <= 0) return null;
	const prev = pos.page - 1;
	return { page: prev, half: landscape(prev) ? 1 : 0 };
}

// Mark-read waits for the LAST half of the last page: the first half of a split
// final page is not the chapter end, so trackers and download cleanup stay correct
export function splitEndReached(
	page: number,
	half: 0 | 1,
	pageCount: number,
	landscape: (page: number) => boolean
): boolean {
	return page >= pageCount - 1 && (!landscape(page) || half === 1);
}
