import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { Chip } from './Chip';

export interface ChipDropdownProps {
	label: ReactNode;
	/** Highlight the chip in its active state */
	active?: boolean;
	/** Receives close so the menu can be dismissed after a pick; multi-select flows simply don't call it to keep the menu open */
	children: (close: () => void) => ReactNode;
}

/** chip dropdown menu: portaled to body + fixed positioning so it is neither clipped by a parent's
 * overflow nor covered by page content; never narrower than the anchor chip and clamped inside the viewport; Escape or any scroll dismisses it */
export function ChipDropdown({ label, active, children }: ChipDropdownProps) {
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<{ top: number; left: number; minWidth: number } | null>(null);
	const anchorRef = useRef<HTMLDivElement>(null);
	const menuRef = useRef<HTMLDivElement>(null);

	function toggle() {
		if (open) {
			setOpen(false);
			return;
		}
		const rect = anchorRef.current?.getBoundingClientRect();
		if (!rect) return;
		// At least as wide as the anchor, and never overflowing the viewport's right edge
		const minWidth = Math.max(rect.width, 160);
		const left = Math.max(8, Math.min(rect.left, window.innerWidth - minWidth - 8));
		setPos({ top: rect.bottom + 4, left, minWidth });
		setOpen(true);
	}

	useEffect(() => {
		if (!open) return;
		const onDown = (e: MouseEvent) => {
			// The menu is portaled under body, not inside the anchor, so the menu must count as inside too —
			// otherwise a menu item's mousedown dismisses the menu first and its onClick never fires
			const t = e.target as Node;
			if (!anchorRef.current?.contains(t) && !menuRef.current?.contains(t)) setOpen(false);
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') setOpen(false);
		};
		// Any scroll dismisses (fixed positioning does not follow the anchor); scrolling inside the menu itself is exempt, else scrolling the menu would close it
		const onScroll = (e: Event) => {
			if (menuRef.current?.contains(e.target as Node)) return;
			setOpen(false);
		};
		document.addEventListener('mousedown', onDown);
		document.addEventListener('keydown', onKey);
		document.addEventListener('scroll', onScroll, true);
		return () => {
			document.removeEventListener('mousedown', onDown);
			document.removeEventListener('keydown', onKey);
			document.removeEventListener('scroll', onScroll, true);
		};
	}, [open]);

	return (
		<div className="chip-dropdown" ref={anchorRef}>
			<Chip color={active ? 'accent' : 'default'} onClick={toggle}>
				{label}
				{open ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
			</Chip>
			{open &&
				pos &&
				createPortal(
					<div
						ref={menuRef}
						className="chip-dropdown__menu"
						style={{ top: pos.top, left: pos.left, minWidth: pos.minWidth }}
					>
						{children(() => setOpen(false))}
					</div>,
					document.body
				)}
		</div>
	);
}
