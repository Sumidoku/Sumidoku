import { useLayoutEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';

export interface SegmentedOption<T extends string | number> {
	value: T;
	label: ReactNode;
}

export interface SegmentedControlProps<T extends string | number> {
	options: SegmentedOption<T>[];
	value: T;
	onChange: (v: T) => void;
}

/** Pill segmented control; the thumb is positioned by the selected item's actual rendered width/offset */
export function SegmentedControl<T extends string | number>({
	options,
	value,
	onChange
}: SegmentedControlProps<T>) {
	const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
	const [thumb, setThumb] = useState<{ left: number; width: number } | null>(null);

	useLayoutEffect(() => {
		const update = () => {
			const i = options.findIndex((o) => o.value === value);
			const el = i >= 0 ? itemRefs.current[i] : null;
			setThumb(el ? { left: el.offsetLeft, width: el.offsetWidth } : null);
		};
		update();
		window.addEventListener('resize', update);
		return () => window.removeEventListener('resize', update);
	}, [value, options]);

	return (
		<div className="seg" role="tablist">
			{thumb && <span className="seg__thumb" style={{ left: thumb.left, width: thumb.width }} />}
			{options.map((o, i) => (
				<button
					key={String(o.value)}
					type="button"
					role="tab"
					aria-selected={o.value === value}
					ref={(el) => {
						itemRefs.current[i] = el;
					}}
					className={o.value === value ? 'seg__item seg__item--active' : 'seg__item'}
					onClick={() => onChange(o.value)}
				>
					{o.label}
				</button>
			))}
		</div>
	);
}
