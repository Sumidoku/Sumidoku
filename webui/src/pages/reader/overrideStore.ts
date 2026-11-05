import type { Dir, Mode, PageScale } from './readerPrefs';
import { parseScale } from './readerPrefs';
import type { SpreadLayout } from './spreads';
import { parseFilters } from './filters';
import type { ReaderFilters } from './filters';

/**
 * Per-manga reader overrides in localStorage, keyed by manga id. Only
 * content-dependent settings carry an override — reading mode, direction,
 * page layout, page scale, small-page stretch, wide-page split and image
 * filters follow the content, so remembering them per manga is what the
 * metadata layer already does for mode and direction. Personal preferences
 * (canvas background, tap zones, turn animation, transition page) are
 * global-only: changing them in the reader writes the global default, same
 * as the settings page, so a change in one manga never leaves another behind.
 *
 * The store is self-cleaning: writing the value the resolution would already
 * produce without an override (the manga's metadata for mode/dir, the global
 * default for the rest) deletes the key instead of storing a redundant entry,
 * clearReaderOverrides drops every key a manga left behind, and
 * sweepGlobalizedOverrides removes the per-manga keys written before the
 * personal preferences went global-only.
 */

const PREFIX = 'sumidoku.reader.';

// Settings that have a per-manga override key
const OVERRIDE_KEYS = ['mode', 'dir', 'layout', 'scale', 'stretch', 'split', 'filters'] as const;
// Settings that had per-manga override keys before they went global-only
const GLOBALIZED_KEYS = ['bg', 'tapzones', 'anim', 'transition'] as const;

function overrideKey(setting: string, mangaId: number): string {
	return `${PREFIX}${setting}.${mangaId}`;
}

function readOverride<T>(setting: string, mangaId: number | null, parse: (v: string | null) => T | null): T | null {
	if (mangaId === null) return null;
	try {
		return parse(localStorage.getItem(overrideKey(setting, mangaId)));
	} catch {
		// In private mode storage is unreadable, so the override reads as absent
	}
	return null;
}

/**
 * Writes an override and returns the override state to keep. Writing the
 * value the resolution would already produce without an override deletes the
 * key, so the store shrinks back as preferences drift to the defaults; a null
 * `resolved` (manga metadata still loading) is never redundant.
 */
function writeOverride<T>(
	setting: string,
	mangaId: number,
	value: T,
	resolved: T | null,
	serialize: (v: T) => string,
	equal: (a: T, b: T) => boolean = (a, b) => a === b
): T | null {
	const redundant = resolved !== null && equal(value, resolved);
	try {
		if (redundant) localStorage.removeItem(overrideKey(setting, mangaId));
		else localStorage.setItem(overrideKey(setting, mangaId), serialize(value));
	} catch {
		// Private mode blocks persistence but the setting still switches
	}
	return redundant ? null : value;
}

function parseMode(v: string | null): Mode | null {
	// 'vertical' is the old settings page's name for scroll; the override keys predate the fix
	return v === 'paged' || v === 'continuous' || v === 'webtoon' ? v : v === 'vertical' ? 'continuous' : null;
}

function parseDir(v: string | null): Dir | null {
	return v === 'ltr' || v === 'rtl' ? v : null;
}

function parseLayout(v: string | null): SpreadLayout | null {
	return v === 'single' || v === 'double' || v === 'doubleNoCover' ? v : null;
}

function parseFlag(v: string | null): boolean | null {
	return v === '1' ? true : v === '0' ? false : null;
}

function serializeFlag(v: boolean): string {
	return v ? '1' : '0';
}

// Mode and direction resolve against the manga's metadata, so `resolved` is
// the metadata value (null while unknown): matching it drops the override so
// future metadata edits apply again
export function readModeOverride(mangaId: number | null): Mode | null {
	return readOverride('mode', mangaId, parseMode);
}

export function writeModeOverride(mangaId: number, mode: Mode, meta: Mode | null): Mode | null {
	return writeOverride('mode', mangaId, mode, meta, (m) => m);
}

export function readDirOverride(mangaId: number | null): Dir | null {
	return readOverride('dir', mangaId, parseDir);
}

export function writeDirOverride(mangaId: number, dir: Dir, meta: Dir | null): Dir | null {
	return writeOverride('dir', mangaId, dir, meta, (d) => d);
}

// The remaining content settings have no metadata counterpart, so `resolved`
// is the global default
export function readLayoutOverride(mangaId: number | null): SpreadLayout | null {
	return readOverride('layout', mangaId, parseLayout);
}

export function writeLayoutOverride(mangaId: number, layout: SpreadLayout, global: SpreadLayout): SpreadLayout | null {
	return writeOverride('layout', mangaId, layout, global, (l) => l);
}

export function readScaleOverride(mangaId: number | null): PageScale | null {
	return readOverride('scale', mangaId, parseScale);
}

export function writeScaleOverride(mangaId: number, scale: PageScale, global: PageScale): PageScale | null {
	return writeOverride('scale', mangaId, scale, global, (s) => s);
}

export function readStretchOverride(mangaId: number | null): boolean | null {
	return readOverride('stretch', mangaId, parseFlag);
}

export function writeStretchOverride(mangaId: number, stretch: boolean, global: boolean): boolean | null {
	return writeOverride('stretch', mangaId, stretch, global, serializeFlag);
}

export function readSplitOverride(mangaId: number | null): boolean | null {
	return readOverride('split', mangaId, parseFlag);
}

export function writeSplitOverride(mangaId: number, split: boolean, global: boolean): boolean | null {
	return writeOverride('split', mangaId, split, global, serializeFlag);
}

// Filters are objects; both sides derive from DEFAULT_FILTERS, so key order is
// stable and JSON equality is exact
export function readFiltersOverride(mangaId: number | null): ReaderFilters | null {
	return readOverride('filters', mangaId, parseFilters);
}

export function writeFiltersOverride(mangaId: number, filters: ReaderFilters, global: ReaderFilters): ReaderFilters | null {
	return writeOverride('filters', mangaId, filters, global, JSON.stringify, (a, b) => JSON.stringify(a) === JSON.stringify(b));
}

/** Drops every per-manga override key a manga left behind, e.g. when it leaves the library. */
export function clearReaderOverrides(mangaId: number): void {
	try {
		for (const setting of [...OVERRIDE_KEYS, ...GLOBALIZED_KEYS]) {
			localStorage.removeItem(overrideKey(setting, mangaId));
		}
	} catch {
		// Storage unavailable: nothing to clean
	}
}

/**
 * One-time sweep for the settings that went global-only: their per-manga keys
 * are no longer read, so keeping them would only grow the store.
 */
export function sweepGlobalizedOverrides(): void {
	try {
		const doomed: string[] = [];
		for (let i = 0; i < localStorage.length; i++) {
			const key = localStorage.key(i);
			if (key !== null && GLOBALIZED_KEYS.some((setting) => key.startsWith(`${PREFIX}${setting}.`))) {
				doomed.push(key);
			}
		}
		for (const key of doomed) localStorage.removeItem(key);
	} catch {
		// Storage unavailable: nothing to sweep
	}
}
