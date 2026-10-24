import { Check, Minus } from 'lucide-react';
import type { ReactNode } from 'react';

export type ChipColor = 'default' | 'accent' | 'danger' | 'success' | 'warning';

export interface ChipProps {
	color?: ChipColor;
	triState?: 1 | -1 | 0;
	/** Renders as a button when onClick is given (clickable multi-select), otherwise a plain label */
	onClick?: () => void;
	title?: string;
	/** Extra class names appended to the chip (e.g. the tri-state excluded state .chip--exclude) */
	className?: string;
	children: ReactNode;
}

export function Chip({ color = 'default', triState, onClick, title, className, children }: ChipProps) {
	const chipColor = triState === undefined ? color : triState === 1 ? 'accent' : triState === -1 ? 'danger' : 'default';
	const cls = [
		'chip',
		chipColor !== 'default' ? `chip--${chipColor}` : '',
		triState === -1 ? 'chip--exclude' : '',
		onClick ? 'chip--clickable' : '',
		className ?? ''
	]
		.filter(Boolean)
		.join(' ');
	const triMarker =
		triState === undefined ? null : (
			<span className="chip__tri-state" aria-hidden="true">
				{triState === 1 && <Check size={12} />}
				{triState === -1 && <Minus size={12} />}
			</span>
		);
	if (onClick) {
		return (
			<button type="button" className={cls} title={title} onClick={onClick}>
				{children}
				{triMarker}
			</button>
		);
	}
	return (
		<span className={cls} title={title}>
			{children}
			{triMarker}
		</span>
	);
}
