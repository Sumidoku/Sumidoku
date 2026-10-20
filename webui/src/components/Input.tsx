import type { InputHTMLAttributes, ReactNode } from 'react';

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
	/** Left icon slot (search boxes take the Search icon) */
	icon?: ReactNode;
}

export function Input({ icon, className, ...rest }: InputProps) {
	const cls = [
		'input',
		icon ? 'input--icon' : '',
		className ?? ''
	]
		.filter(Boolean)
		.join(' ');
	return (
		<span className={cls}>
			{icon && <span className="input__icon">{icon}</span>}
			<input className="input__field" {...rest} />
		</span>
	);
}
