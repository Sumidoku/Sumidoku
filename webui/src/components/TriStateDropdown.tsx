import { Check, X } from 'lucide-react';
import { ChipDropdown } from './ChipDropdown';

export interface TriStateDropdownProps {
	label: string;
	options: { key: string; label: string }[];
	/** Tri-state per option: 1 include / -1 exclude / absent = unselected */
	states: Record<string, 1 | -1>;
	onCycle: (key: string) => void;
}

/** Tri-state multi-select dropdown: the chip shows the selection count, clicking a menu item cycles unselected → include → exclude;
 * the menu stays open (Aidoku menuActionDismissDisabled) for consecutive multi-select */
export function TriStateDropdown({ label, options, states, onCycle }: TriStateDropdownProps) {
	const count = Object.keys(states).length;
	return (
		<ChipDropdown active={count > 0} label={`${label}${count > 0 ? `（${count}）` : ''}`}>
			{() =>
				options.map((opt) => {
					const st = states[opt.key] ?? 0;
					return (
						<button
							key={opt.key}
							type="button"
							className="chip-dropdown__item"
							onClick={() => onCycle(opt.key)}
						>
							<span>
								{st === -1 ? '− ' : ''}
								{opt.label}
							</span>
							{st === 1 && <Check size={14} />}
							{st === -1 && <X size={14} />}
						</button>
					);
				})
			}
		</ChipDropdown>
	);
}
