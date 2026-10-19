import { Check, X } from 'lucide-react';
import { ChipDropdown } from './ChipDropdown';

export interface TriStateDropdownProps {
	label: string;
	options: { key: string; label: string }[];
	/** 每个选项的三态：1 包含 / -1 排除 / 缺省未选 */
	states: Record<string, 1 | -1>;
	onCycle: (key: string) => void;
}

/** 三态多选下拉：chip 展示选中计数，菜单项点击循环 未选 → 包含 → 排除；
 * 菜单不收起（Aidoku menuActionDismissDisabled），连续多选 */
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
