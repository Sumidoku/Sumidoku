import type { ButtonHTMLAttributes, ReactNode } from 'react';

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
	/** Icon buttons have no visible text; label serves only as the accessible name */
	label: string;
	variant?: 'ghost' | 'danger';
	size?: 'sm' | 'md';
	children: ReactNode;
}

export function IconButton({
	label,
	variant = 'ghost',
	size = 'md',
	className,
	children,
	...rest
}: IconButtonProps) {
	const cls = [
		'icon-btn',
		variant === 'danger' ? 'icon-btn--danger' : '',
		size === 'sm' ? 'icon-btn--sm' : '',
		className ?? ''
	]
		.filter(Boolean)
		.join(' ');
	return (
		<button type="button" className={cls} aria-label={label} title={label} {...rest}>
			{children}
		</button>
	);
}
