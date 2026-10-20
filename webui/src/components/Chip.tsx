import type { ReactNode } from 'react';

export type ChipColor = 'default' | 'accent' | 'danger' | 'success' | 'warning';

export interface ChipProps {
	color?: ChipColor;
	/** Renders as a button when onClick is given (clickable multi-select), otherwise a plain label */
	onClick?: () => void;
	title?: string;
	/** Extra class names appended to the chip (e.g. the tri-state excluded state .chip--exclude) */
	className?: string;
	children: ReactNode;
}

export function Chip({ color = 'default', onClick, title, className, children }: ChipProps) {
	const cls = [
		'chip',
		color !== 'default' ? `chip--${color}` : '',
		onClick ? 'chip--clickable' : '',
		className ?? ''
	]
		.filter(Boolean)
		.join(' ');
	if (onClick) {
		return (
			<button type="button" className={cls} title={title} onClick={onClick}>
				{children}
			</button>
		);
	}
	return (
		<span className={cls} title={title}>
			{children}
		</span>
	);
}
