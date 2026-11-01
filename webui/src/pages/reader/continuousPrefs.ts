import type { CSSProperties } from 'react';
import type { PageDims } from './spreads';

export type ContinuousScale = 'width' | 'original';

export function parseContinuousScale(value: string | null): ContinuousScale | null {
	return value === 'width' || value === 'original' ? value : null;
}

export function readBoundedNumber(value: string | null, max: number): number | null {
	if (value === null || value.trim() === '') return null;
	const n = Number(value);
	return Number.isInteger(n) && n >= 0 && n <= max ? n : null;
}

export function continuousPageStyle(
	mode: 'continuous' | 'webtoon',
	scale: ContinuousScale,
	padding: number,
	gap: number,
	first: boolean,
	dims: PageDims | null
): CSSProperties {
	const naturalWidth = scale === 'original' && dims !== null ? `${dims.width}px` : null;
	return {
		width: naturalWidth ?? `calc(${100 - padding * 2}% - ${mode === 'continuous' ? 16 : 0}px)`,
		paddingTop: first ? 0 : gap,
		aspectRatio: dims ? `${dims.width} / ${dims.height}` : undefined
	};
}
