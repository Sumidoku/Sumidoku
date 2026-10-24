import { Check, RotateCcw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ChipDropdown } from './ChipDropdown';

export interface TriStateDropdownProps {
	label: string;
	options: { key: string; label: string }[];
	/** Tri-state per option: 1 include / -1 exclude / absent = unselected */
	states: Record<string, 1 | -1>;
	onCycle: (key: string) => void;
	/** Clear every selection in this dimension; the reset row only renders when this callback is provided */
	onReset?: () => void;
}

/** Tri-state multi-select dropdown: the chip shows the selection count, clicking a menu item cycles unselected → include → exclude;
 * the menu stays open (Aidoku menuActionDismissDisabled) for consecutive multi-select. When onReset is supplied, a reset row
 * appears at the top and closes the menu after clearing. */
export function TriStateDropdown({ label, options, states, onCycle, onReset }: TriStateDropdownProps) {
	const { t } = useTranslation();
	const count = Object.keys(states).length;
	const canReset = onReset !== undefined && count > 0;
	return (
		<ChipDropdown active={count > 0} label={`${label}${count > 0 ? `（${count}）` : ''}`}>
			{(close) => (
				<>
					{canReset && (
						<button
							type="button"
							className="chip-dropdown__item chip-dropdown__item--reset"
							onClick={() => {
								onReset();
								close();
							}}
						>
							<span>{t('common.reset')}</span>
							<RotateCcw size={14} />
						</button>
					)}
					{options.map((opt) => {
						const st = states[opt.key] ?? 0;
						return (
							<button
								key={opt.key}
								type="button"
								className="chip-dropdown__item"
								onClick={() => onCycle(opt.key)}
							>
								<span>{opt.label}</span>
								<span className="chip-dropdown__state" aria-hidden="true">
									{st === 1 && <Check size={14} />}
									{st === -1 && <X size={14} />}
								</span>
							</button>
						);
					})}
				</>
			)}
		</ChipDropdown>
	);
}
