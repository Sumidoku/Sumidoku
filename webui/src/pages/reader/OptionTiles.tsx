import type { ReactNode } from 'react';

interface OptionTilesProps<T extends string> {
	label: string;
	options: ReadonlyArray<{ value: T; label: string; icon?: ReactNode; preview?: ReactNode }>;
	value: T;
	onChange: (value: T) => void;
	columns?: number;
}

export function OptionTiles<T extends string>({ label, options, value, onChange, columns = 3 }: OptionTilesProps<T>) {
	return (
		<div className="reader__option-tiles" role="group" aria-label={label} style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}>
			{options.map((option) => (
				<button
					type="button"
					key={option.value}
					className={option.value === value ? 'reader__option reader__option--selected' : 'reader__option'}
					aria-pressed={option.value === value}
					onClick={() => onChange(option.value)}
				>
					{option.preview ?? (option.icon && <span className="reader__option-icon">{option.icon}</span>)}
					<span>{option.label}</span>
				</button>
			))}
		</div>
	);
}
