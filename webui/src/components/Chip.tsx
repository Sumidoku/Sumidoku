import type { ReactNode } from 'react';

export type ChipColor = 'default' | 'accent' | 'danger' | 'success' | 'warning';

export interface ChipProps {
	color?: ChipColor;
	/** 传了 onClick 就渲染成按钮（可点多选），否则是纯标签 */
	onClick?: () => void;
	title?: string;
	children: ReactNode;
}

export function Chip({ color = 'default', onClick, title, children }: ChipProps) {
	const cls = ['chip', color !== 'default' ? `chip--${color}` : '', onClick ? 'chip--clickable' : '']
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
