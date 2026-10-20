import type { ReactNode, SelectHTMLAttributes } from 'react';
import { ChevronDown } from 'lucide-react';

export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
	children: ReactNode;
}

/** Native select wrapper with a chevron, keeping the native keyboard and mobile experience */
export function Select({ children, className, ...rest }: SelectProps) {
	return (
		<span className={className ? `select ${className}` : 'select'}>
			<select className="select__field" {...rest}>
				{children}
			</select>
			<span className="select__chevron">
				<ChevronDown size={14} />
			</span>
		</span>
	);
}
