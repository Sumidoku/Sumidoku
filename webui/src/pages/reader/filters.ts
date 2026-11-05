// Reader image filters, ported from Suwayomi's ReaderCustomFilter
// (Suwayomi-WebUI src/features/reader/Reader.types.ts, MPL-2.0).
// Pure logic module: types, defaults, parsing, and CSS generation.
// Persistence wiring lives in ReaderPage.tsx next to the other reader prefs.

export type BlendMode =
	| 'normal'
	| 'multiply'
	| 'screen'
	| 'overlay'
	| 'darken'
	| 'lighten';

export const BLEND_MODES: readonly BlendMode[] = [
	'normal',
	'multiply',
	'screen',
	'overlay',
	'darken',
	'lighten',
];

export interface FilterLevel {
	enabled: boolean;
	value: number;
}

export interface RgbaFilter {
	enabled: boolean;
	red: number;
	green: number;
	blue: number;
	alpha: number;
	blendMode: BlendMode;
}

export interface ReaderFilters {
	brightness: FilterLevel;
	contrast: FilterLevel;
	saturate: FilterLevel;
	hue: FilterLevel;
	sepia: boolean;
	grayscale: boolean;
	invert: boolean;
	rgba: RgbaFilter;
}

export interface FilterLimit {
	min: number;
	max: number;
	step: number;
	defaultValue: number;
}

// Numeric levels. Suwayomi's hue slider stops at 200; we expose the full
// color wheel (0-360deg) since hue-rotate has no natural ceiling at 200.
export const FILTER_LIMITS: Record<
	'brightness' | 'contrast' | 'saturate' | 'hue',
	FilterLimit
> = {
	brightness: { min: 5, max: 200, step: 1, defaultValue: 100 },
	contrast: { min: 5, max: 200, step: 1, defaultValue: 100 },
	saturate: { min: 0, max: 200, step: 1, defaultValue: 100 },
	hue: { min: 0, max: 360, step: 1, defaultValue: 0 },
};

export type FilterLevelKey = keyof typeof FILTER_LIMITS;

export const RGBA_CHANNEL_LIMIT: FilterLimit = {
	min: 0,
	max: 255,
	step: 1,
	defaultValue: 0,
};

export const RGBA_ALPHA_LIMIT: FilterLimit = {
	min: 0,
	max: 100,
	step: 1,
	defaultValue: 50,
};

export const DEFAULT_FILTERS: ReaderFilters = {
	brightness: { enabled: false, value: FILTER_LIMITS.brightness.defaultValue },
	contrast: { enabled: false, value: FILTER_LIMITS.contrast.defaultValue },
	saturate: { enabled: false, value: FILTER_LIMITS.saturate.defaultValue },
	hue: { enabled: false, value: FILTER_LIMITS.hue.defaultValue },
	sepia: false,
	grayscale: false,
	invert: false,
	rgba: {
		enabled: false,
		red: 0,
		green: 0,
		blue: 0,
		alpha: RGBA_ALPHA_LIMIT.defaultValue,
		blendMode: 'normal',
	},
};

function clampInt(value: number, limit: FilterLimit): number {
	const clamped = Math.min(limit.max, Math.max(limit.min, value));
	return Math.round(clamped / limit.step) * limit.step;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseLevel(raw: unknown, limit: FilterLimit): FilterLevel | null {
	if (!isPlainObject(raw)) return null;
	if (typeof raw.enabled !== 'boolean') return null;
	if (typeof raw.value !== 'number' || !Number.isFinite(raw.value)) return null;
	return { enabled: raw.enabled, value: clampInt(raw.value, limit) };
}

function parseRgba(raw: unknown): RgbaFilter | null {
	if (!isPlainObject(raw)) return null;
	// Missing sub-fields keep defaults, so blobs stored before a schema grows
	// still parse; only present-but-wrong values reject the whole blob
	const enabled = raw.enabled;
	if (enabled !== undefined && typeof enabled !== 'boolean') return null;
	const parsed: RgbaFilter = { ...DEFAULT_FILTERS.rgba };
	if (enabled !== undefined) parsed.enabled = enabled;
	const channels: Array<'red' | 'green' | 'blue'> = ['red', 'green', 'blue'];
	for (const channel of channels) {
		const value = raw[channel];
		if (value === undefined) continue;
		if (typeof value !== 'number' || !Number.isFinite(value)) return null;
		parsed[channel] = clampInt(value, RGBA_CHANNEL_LIMIT);
	}
	if (raw.alpha !== undefined) {
		if (typeof raw.alpha !== 'number' || !Number.isFinite(raw.alpha)) return null;
		parsed.alpha = clampInt(raw.alpha, RGBA_ALPHA_LIMIT);
	}
	if (raw.blendMode !== undefined) {
		if (typeof raw.blendMode !== 'string' || !BLEND_MODES.includes(raw.blendMode as BlendMode)) {
			return null;
		}
		parsed.blendMode = raw.blendMode as BlendMode;
	}
	return parsed;
}

// Whitelist parse following the readerPrefs convention: malformed stored
// values (bad JSON, wrong types, unknown blend modes) read as absent so the
// reader falls back to defaults. Missing fields fall back to DEFAULT_FILTERS,
// out-of-range numbers are clamped into their slider limits.
export function parseFilters(json: string | null): ReaderFilters | null {
	if (!json) return null;
	let raw: unknown;
	try {
		raw = JSON.parse(json);
	} catch {
		return null;
	}
	if (!isPlainObject(raw)) return null;

	const filters: ReaderFilters = {
		brightness: { ...DEFAULT_FILTERS.brightness },
		contrast: { ...DEFAULT_FILTERS.contrast },
		saturate: { ...DEFAULT_FILTERS.saturate },
		hue: { ...DEFAULT_FILTERS.hue },
		sepia: DEFAULT_FILTERS.sepia,
		grayscale: DEFAULT_FILTERS.grayscale,
		invert: DEFAULT_FILTERS.invert,
		rgba: { ...DEFAULT_FILTERS.rgba },
	};

	for (const key of Object.keys(FILTER_LIMITS) as FilterLevelKey[]) {
		if (raw[key] === undefined) continue;
		const level = parseLevel(raw[key], FILTER_LIMITS[key]);
		if (level === null) return null;
		filters[key] = level;
	}

	for (const key of ['sepia', 'grayscale', 'invert'] as const) {
		if (raw[key] === undefined) continue;
		if (typeof raw[key] !== 'boolean') return null;
		filters[key] = raw[key];
	}

	if (raw.rgba !== undefined) {
		const rgba = parseRgba(raw.rgba);
		if (rgba === null) return null;
		filters.rgba = rgba;
	}

	return filters;
}

/** id of the SVG filter that paints the RGBA tint; ReaderPage renders its
    definition (TintFilterDefs) whenever the tint is enabled */
export const TINT_FILTER_ID = 'reader-tint';

// CSS `filter` value for image pages. CSS has no tint-with-blend-mode filter
// function, so the RGBA tint joins the chain as a reference to an SVG filter
// (feFlood clipped to the image alpha, then feBlend — after kmrs #360). It
// runs last, on top of the other adjustments, and touches only the image's
// own pixels: the canvas letterbox and transparent areas stay untouched.
export function buildFilterString(filters: ReaderFilters): string {
	const parts: string[] = [];
	if (filters.brightness.enabled) {
		parts.push(`brightness(${filters.brightness.value / 100})`);
	}
	if (filters.contrast.enabled) {
		parts.push(`contrast(${filters.contrast.value / 100})`);
	}
	if (filters.saturate.enabled) {
		parts.push(`saturate(${filters.saturate.value / 100})`);
	}
	if (filters.hue.enabled) {
		parts.push(`hue-rotate(${filters.hue.value}deg)`);
	}
	if (filters.invert) parts.push('invert(1)');
	if (filters.sepia) parts.push('sepia(1)');
	if (filters.grayscale) parts.push('grayscale(1)');
	if (filters.rgba.enabled) parts.push(`url(#${TINT_FILTER_ID})`);
	return parts.join(' ');
}

// True when any filter deviates from the all-off default. Drives the
// settings panel's reset button and keeps the CSS variable unset when
// nothing is active.
export function hasActiveFilters(filters: ReaderFilters): boolean {
	return (
		filters.brightness.enabled ||
		filters.contrast.enabled ||
		filters.saturate.enabled ||
		filters.hue.enabled ||
		filters.sepia ||
		filters.grayscale ||
		filters.invert ||
		filters.rgba.enabled
	);
}
