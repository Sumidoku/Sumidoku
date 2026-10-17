import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { Spinner } from './Spinner';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
	variant?: 'primary' | 'ghost' | 'danger';
	size?: 'sm' | 'md';
	loading?: boolean;
	children: ReactNode;
}

export function Button({
	variant = 'ghost',
	size = 'md',
	loading = false,
	disabled,
	className,
	children,
	...rest
}: ButtonProps) {
	const cls = [
		'btn',
		variant === 'primary' ? 'btn--primary' : '',
		variant === 'danger' ? 'btn--danger' : '',
		size === 'sm' ? 'btn--sm' : '',
		className ?? ''
	]
		.filter(Boolean)
		.join(' ');
	return (
		<button type="button" className={cls} disabled={disabled || loading} {...rest}>
			{loading && <Spinner />}
			{children}
		</button>
	);
}
