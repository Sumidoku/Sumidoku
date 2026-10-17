export interface SwitchProps {
	checked: boolean;
	onChange: (checked: boolean) => void;
	disabled?: boolean;
	label?: string;
}

export function Switch({ checked, onChange, disabled, label }: SwitchProps) {
	return (
		<input
			type="checkbox"
			className="switch"
			checked={checked}
			disabled={disabled}
			aria-label={label}
			onChange={(e) => onChange(e.currentTarget.checked)}
		/>
	);
}
