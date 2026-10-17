import type { CSSProperties } from 'react';

export interface SkeletonProps {
	width?: number | string;
	height?: number | string;
	/** 例 '2 / 3'，用于封面位骨架 */
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
