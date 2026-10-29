import type { Viewer } from '../../api/types';

export type Dir = 'ltr' | 'rtl';
export type Mode = 'paged' | 'continuous' | 'webtoon';

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
 * Reading mode implied by manga metadata. Aidoku's Webtoon is the gapless vertical
 * strip; Vertical is vertical paged, for which continuous is the closest local mode.
 * The horizontal viewers say nothing about which mode to use.
 */
export function modeFromViewer(viewer: Viewer | null | undefined): Mode | null {
	if (viewer === 'Webtoon') return 'webtoon';
	if (viewer === 'Vertical') return 'continuous';
	return null;
}

/** Same resolution order as resolveDir: per-manga override > metadata > global default */
export function resolveMode(override: Mode | null, meta: Mode | null, fallback: Mode): Mode {
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

/**
 * Tap-zone layouts of the paged reader, after Suwayomi's presets. `lr` is the
 * classic prev | menu | next split, `rl` swaps the page-turn sides, `edge` thins
 * the turn zones, `kindle` gives prev most of the screen with the menu on a top
 * strip, and `off` removes the zones (taps fall back to toggling the toolbar).
 */
export type TapZoneLayout = 'lr' | 'rl' | 'edge' | 'kindle' | 'off';

export type ZoneAction = 'prev' | 'next' | 'menu' | 'none';

/**
 * Zone widths as percentages of the reader width. The kindle layout spends its
 * menu zone on a full-width top strip (see zoneAt), so its mid width is 0 here;
 * off renders no zones at all.
 */
export function zoneWidths(layout: TapZoneLayout): { left: number; mid: number; right: number } {
	switch (layout) {
		case 'edge':
			return { left: 15, mid: 70, right: 15 };
		case 'kindle':
			return { left: 75, mid: 0, right: 25 };
		case 'off':
			return { left: 0, mid: 0, right: 0 };
		default:
			return { left: 28, mid: 44, right: 28 };
	}
}

/**
 * The action a tap zone fires. `rl` mirrors the page-turn mapping, so e.g. rl+rtl
 * moves forward on the right again; kindle keeps the `lr` side mapping. The mid
 * zone always summons the toolbar, and off disables every zone.
 */
export function zoneAction(dir: Dir, layout: TapZoneLayout, which: 'left' | 'right' | 'mid'): ZoneAction {
	if (layout === 'off') return 'none';
	if (which === 'mid') return 'menu';
	return zoneMovesForward(dir, which) !== (layout === 'rl') ? 'next' : 'prev';
}

/**
 * Physical hit-test for a tap on the paged container, mirroring the rendered zone
 * geometry (reader.css `[data-tz]` rules). kindle puts the menu on a full-width
 * top strip and splits the rest between prev (75%) and next (25%); off maps every
 * tap to the menu so the toolbar stays reachable.
 */
export function zoneAt(layout: TapZoneLayout, xRatio: number, yRatio: number): 'left' | 'right' | 'mid' {
	if (layout === 'off') return 'mid';
	if (layout === 'kindle') {
		if (yRatio < 0.25) return 'mid';
		return xRatio < 0.75 ? 'left' : 'right';
	}
	const { left, right } = zoneWidths(layout);
	if (xRatio < left / 100) return 'left';
	if (xRatio > 1 - right / 100) return 'right';
	return 'mid';
}

/** Reader canvas background behind the pages. */
export type ReaderBackground = 'dark' | 'gray' | 'light';

/** Whitelisted parse of a stored tap-zone value; unknown values read as absent. */
export function parseTapZones(v: string | null): TapZoneLayout | null {
	return v === 'lr' || v === 'rl' || v === 'edge' || v === 'kindle' || v === 'off' ? v : null;
}

/** Whitelisted parse of a stored background value; unknown values read as absent. */
export function parseBackground(v: string | null): ReaderBackground | null {
	return v === 'dark' || v === 'gray' || v === 'light' ? v : null;
}
