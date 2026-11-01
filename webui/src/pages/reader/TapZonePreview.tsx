import { zoneAction, zoneAt } from './readerPrefs';
import type { Dir, TapZoneLayout } from './readerPrefs';

const COLUMNS = 20;
const ROWS = 8;

export function TapZonePreview({ layout, dir }: { layout: TapZoneLayout; dir: Dir }) {
	const cells = Array.from({ length: COLUMNS * ROWS }, (_, i) => {
		const x = i % COLUMNS;
		const y = Math.floor(i / COLUMNS);
		const action = zoneAction(dir, layout, zoneAt(layout, (x + 0.5) / COLUMNS, (y + 0.5) / ROWS));
		return <rect key={i} x={x} y={y} width={1} height={1} className={`reader__tz-fill--${action}`} />;
	});
	return (
		<svg className="reader__tz-preview" viewBox={`0 0 ${COLUMNS} ${ROWS}`} preserveAspectRatio="none" shapeRendering="crispEdges" aria-hidden>
			{cells}
		</svg>
	);
}
