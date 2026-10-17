import type { ReactNode, SelectHTMLAttributes } from 'react';
import { ChevronDown } from 'lucide-react';

export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
	children: ReactNode;
}

/** 带箭头的原生 select 包装，保留键盘与移动端原生体验 */
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
