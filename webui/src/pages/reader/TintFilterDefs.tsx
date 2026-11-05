import { TINT_FILTER_ID } from './filters';
import type { RgbaFilter } from './filters';

/** Definition of the SVG filter that page images reference for the color tint.
    The flood is clipped to the image's alpha before blending, so letterboxing
    and transparent areas stay untouched; feBlend composites like
    mix-blend-mode, matching the blended overlay it replaces (kmrs #360) */
export function TintFilterDefs({ tint }: { tint: RgbaFilter }) {
	return (
		<svg aria-hidden width="0" height="0" style={{ position: 'absolute', pointerEvents: 'none' }}>
			<filter id={TINT_FILTER_ID} x="0" y="0" width="1" height="1" colorInterpolationFilters="sRGB">
				<feFlood floodColor={`rgb(${tint.red}, ${tint.green}, ${tint.blue})`} floodOpacity={tint.alpha / 100} />
				<feComposite in2="SourceAlpha" operator="in" />
				<feBlend in2="SourceGraphic" mode={tint.blendMode} />
			</filter>
		</svg>
	);
}
