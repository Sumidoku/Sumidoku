import { Ban, Check } from 'lucide-react';
import type { ReactNode } from 'react';

export type ChipColor = 'default' | 'accent' | 'danger' | 'success' | 'warning';

export interface ChipProps {
	color?: ChipColor;
	triState?: 1 | -1 | 0;
	/** Renders as a button when onClick is given (clickable multi-select), otherwise a plain label */
	onClick?: () => void;
	title?: string;
	className?: string;
	children: ReactNode;
}

export function Chip({ color = 'default', triState, onClick, title, className, children }: ChipProps) {
	const chipColor = triState === undefined ? color : triState === 1 ? 'accent' : triState === -1 ? 'danger' : 'default';
	const cls = [
		'chip',
		chipColor !== 'default' ? `chip--${chipColor}` : '',
		onClick ? 'chip--clickable' : '',
		className ?? ''
	]
		.filter(Boolean)
		.join(' ');
	const triMarker =
		triState === 1 || triState === -1 ? (
			<span className="chip__tri-state" aria-hidden="true">
				{triState === 1 ? <Check size={12} /> : <Ban size={12} />}
			</span>
		) : null;
	if (onClick) {
		return (
			<button type="button" className={cls} title={title} onClick={onClick}>
				{triMarker}
				{children}
			</button>
		);
	}
	return (
		<span className={cls} title={title}>
			{triMarker}
			{children}
		</span>
	);
}
