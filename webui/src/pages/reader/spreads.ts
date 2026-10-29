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
