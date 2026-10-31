import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { Chip } from './Chip';
import { useAnchoredMenu } from './useAnchoredMenu';

export interface ChipDropdownProps {
	label: ReactNode;
	/** Highlight the chip in its active state */
	active?: boolean;
	/** Receives close so the menu can be dismissed after a pick; multi-select flows simply don't call it to keep the menu open */
	children: (close: () => void) => ReactNode;
}

/** chip dropdown menu, never narrower than the anchor chip */
export function ChipDropdown({ label, active, children }: ChipDropdownProps) {
	const { open, style, anchorRef, menuRef, show, close } = useAnchoredMenu<HTMLDivElement>();

	return (
		<div className="chip-dropdown" ref={anchorRef}>
			<Chip color={active ? 'accent' : 'default'} onClick={open ? close : show}>
				{label}
				{open ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
			</Chip>
			{open &&
				createPortal(
					<div ref={menuRef} className="chip-dropdown__menu" style={style}>
						{children(close)}
					</div>,
					document.body
				)}
		</div>
	);
}
