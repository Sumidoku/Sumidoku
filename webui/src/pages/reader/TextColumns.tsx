import type { CSSProperties, RefObject } from 'react';
import { textColumnOffset } from './readerPrefs';
import type { Dir } from './readerPrefs';

export interface TextLayout {
	w: number;
	gap: number;
	cols: number;
}

interface TextColumnsProps {
	dir: Dir;
	html: string;
	textStyle: CSSProperties;
	layout: TextLayout;
	col: number;
	// Only the current spread's columns carry the measurement ref; neighbours omit it
	innerRef?: RefObject<HTMLDivElement | null>;
}

// Paged text page split into viewport-wide columns; the column flow and the track's
// translate direction both follow the reading direction
export function TextColumns({ dir, html, textStyle, layout, col, innerRef }: TextColumnsProps) {
	return (
		<div className="reader__text-paged">
			<div
				className="reader-text reader-text--cols"
				ref={innerRef}
				style={{
					...textStyle,
					direction: dir === 'rtl' ? 'rtl' : undefined,
					columnWidth: layout.w > 0 ? `${layout.w}px` : undefined,
					transform: `translateX(${textColumnOffset(dir, col, layout.w, layout.gap)}px)`
				}}
				dangerouslySetInnerHTML={{ __html: html }}
			/>
		</div>
	);
}
