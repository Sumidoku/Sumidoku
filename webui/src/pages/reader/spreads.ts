/**
 * Double-page spread building for the paged reader, ported from KMReader's
 * spreads.ts. A spread holds page indexes in reading order; null is a blank
 * filler keeping book conventions (cover and closing page sit on their outer
 * side, landscape pages always get a solo spread).
 */
export type SpreadLayout = 'single' | 'double' | 'doubleNoCover';

/** A spread slot: an index into the chapter's page array, or null for a blank filler. */
export type SpreadSlot = number | null;
export type Spread = SpreadSlot[];

export function isLandscapePage(width: number | undefined, height: number | undefined): boolean {
	return !!width && !!height && width > height;
}

/**
 * 'double' pins a cover spread (page 1 paired with a blank on its outer side) and a
 * closing blank on the last page; 'doubleNoCover' pairs straight from the first page.
 * Landscape pages always stand alone; when the second page of a pair is landscape the
 * first page gets a blank mate instead.
 */
export function buildSpreads(
	pageCount: number,
	isLandscape: (index: number) => boolean,
	layout: SpreadLayout
): Spread[] {
	if (pageCount <= 0) return [];
	if (layout === 'single') return Array.from({ length: pageCount }, (_, i) => [i]);

	const queue = Array.from({ length: pageCount }, (_, i) => i);
	const spreads: Spread[] = [];
	let lastSpread: Spread | undefined;

	if (layout === 'double') {
		const first = queue.shift()!;
		spreads.push(isLandscape(first) ? [first] : [null, first]);
		if (queue.length > 0) {
			const last = queue.pop()!;
			lastSpread = isLandscape(last) ? [last] : [last, null];
		}
	}

	while (queue.length > 0) {
		const p = queue.shift()!;
		if (isLandscape(p)) {
			spreads.push([p]);
		} else if (queue.length > 0) {
			const p2 = queue.shift()!;
			if (isLandscape(p2)) {
				spreads.push([p, null]);
				spreads.push([p2]);
			} else {
				spreads.push([p, p2]);
			}
		} else {
			spreads.push([p, null]);
		}
	}

	if (lastSpread) spreads.push(lastSpread);
	return spreads;
}

export interface PageDims {
	width: number;
	height: number;
}

export interface MeasurableImage {
	naturalWidth: number;
	naturalHeight: number;
	complete: boolean;
}

/**
 * The natural size settles as soon as the browser has parsed the image header,
 * long before the load event, so polling reports dimensions early enough to fix
 * spread pairing (and wide-page splitting) before the page finishes loading.
 * The poll is capped so a stalled load cannot spin forever; the load handler
 * remains the fallback and recordMeasured deduplicates the double report.
 * The scheduler is injectable so the polling loop is testable without a DOM.
 */
export function measureWhenKnown(
	img: MeasurableImage,
	report: (width: number, height: number) => void,
	schedule: (cb: () => void) => void = (cb) => requestAnimationFrame(() => cb()),
	maxFrames = 600
): void {
	let frames = 0;
	const poll = () => {
		if (img.naturalWidth > 0 && img.naturalHeight > 0) {
			report(img.naturalWidth, img.naturalHeight);
			return;
		}
		if (!img.complete && ++frames < maxFrames) schedule(poll);
	};
	schedule(poll);
}

/**
 * Records dimensions measured from a loaded image. Server-reported dimensions win:
 * a measured value only fills a gap, never overrides. Invalid or identical values
 * return the previous map unchanged so React skips the re-render.
 */
export function recordMeasured(
	prev: Record<number, PageDims>,
	index: number,
	dims: PageDims,
	serverDims: PageDims | null
): Record<number, PageDims> {
	if (serverDims) return prev;
	if (!Number.isFinite(dims.width) || !Number.isFinite(dims.height) || dims.width <= 0 || dims.height <= 0) {
		return prev;
	}
	const old = prev[index];
	if (old && old.width === dims.width && old.height === dims.height) return prev;
	return { ...prev, [index]: dims };
}

/** Index of the spread containing the page (0 when absent, which cannot happen for built spreads). */
export function spreadIndexOf(spreads: Spread[], page: number): number {
	for (let i = 0; i < spreads.length; i++) {
		if (spreads[i].includes(page)) return i;
	}
	return 0;
}

/** The spread's leading page in reading order; every built spread holds at least one real page. */
export function spreadLeadPage(spread: Spread): number {
	for (const slot of spread) {
		if (slot !== null) return slot;
	}
	return 0;
}

/**
 * Track translation in percent for the turn animation. The track lays spreads in
 * reading order; in rtl the flex row runs right to left, so the sign flips to
 * keep the current spread sliding in from the correct side.
 */
export function trackOffsetPercent(dir: 'ltr' | 'rtl', spreadIndex: number): number {
	// Zero must not become -0, or React renders a different style string on the first spread
	if (spreadIndex === 0) return 0;
	return (dir === 'rtl' ? 1 : -1) * spreadIndex * 100;
}
