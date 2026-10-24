import type { CSSProperties } from 'react';

export type ReaderFont = 'default' | 'sans' | 'serif' | 'mono';
export interface ReaderTypography {
	font: ReaderFont;
	fontSize: number;
	lineHeight: number;
}

export const DEFAULT_TYPOGRAPHY: ReaderTypography = { font: 'default', fontSize: 1, lineHeight: 1.8 };
export const FONT_SIZE_LIMITS = { min: 0.7, max: 2, step: 0.05 };
export const LINE_HEIGHT_LIMITS = { min: 1, max: 2.5, step: 0.1 };

const STORAGE_KEY = 'sumidoku.reader.typography';
const FONT_STACKS: Record<Exclude<ReaderFont, 'default'>, string> = {
	sans: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Noto Sans SC', sans-serif",
	serif: "Georgia, 'Songti SC', 'Noto Serif CJK SC', serif",
	mono: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
};

function validNumber(value: unknown, min: number, max: number, fallback: number) {
	return typeof value === 'number' && Number.isFinite(value) && value >= min && value <= max ? value : fallback;
}

export function loadTypography(): ReaderTypography {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return { ...DEFAULT_TYPOGRAPHY };
		const value = JSON.parse(raw) as Partial<ReaderTypography>;
		return {
			font: value.font === 'sans' || value.font === 'serif' || value.font === 'mono' ? value.font : 'default',
			fontSize: validNumber(value.fontSize, FONT_SIZE_LIMITS.min, FONT_SIZE_LIMITS.max, 1),
			lineHeight: validNumber(value.lineHeight, LINE_HEIGHT_LIMITS.min, LINE_HEIGHT_LIMITS.max, 1.8)
		};
	} catch {
		return { ...DEFAULT_TYPOGRAPHY };
	}
}

export function saveTypography(value: ReaderTypography) {
	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
	} catch {
		// Private mode blocks persistence; the current session still applies the setting
	}
}

export function typographyStyle(value: ReaderTypography): CSSProperties {
	return {
		fontFamily: value.font === 'default' ? undefined : FONT_STACKS[value.font],
		fontSize: `${value.fontSize}em`,
		lineHeight: value.lineHeight
	};
}
