import type { CSSProperties } from 'react';

export interface SkeletonProps {
	width?: number | string;
	height?: number | string;
	/** e.g. '2 / 3', for cover-slot skeletons */
	aspect?: string;
	style?: CSSProperties;
	className?: string;
}

export function Skeleton({ width, height, aspect, style, className }: SkeletonProps) {
	const merged: CSSProperties = {
		width,
		height,
		aspectRatio: aspect,
		...style
	};
	return <div className={className ? `skeleton ${className}` : 'skeleton'} style={merged} />;
}
