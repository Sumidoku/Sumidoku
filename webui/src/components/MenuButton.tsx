import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { MoreVertical } from 'lucide-react';
import { IconButton } from './IconButton';

interface MenuButtonProps {
	label: string;
	disabled?: boolean;
	placement?: 'bottom' | 'top';
	/** Entries render through a close callback so an action can dismiss the popover */
	children: (close: () => void) => ReactNode;
}

export function handleMenuToggle(event: { stopPropagation: () => void }, toggle: () => void): void {
	event.stopPropagation();
	toggle();
}

export function isMenuPopupVisible(open: boolean, disabled: boolean): boolean {
	return open && !disabled;
}

export function menuPosition(
	anchor: Pick<DOMRect, 'top' | 'bottom' | 'right'>,
	placement: 'bottom' | 'top',
	viewportHeight: number,
	viewportWidth: number
): { top: number; right: number } | { bottom: number; right: number } {
	const right = viewportWidth - anchor.right;
	return placement === 'top'
		? { bottom: viewportHeight - anchor.top + 6, right }
		: { top: anchor.bottom + 6, right };
}

// Anchored dropdown for row-level action menus (chapter rows, list items). The popover goes through a
// portal because row containers commonly clip absolute positioning; same pattern as the manga page's MoreMenu.
export function MenuButton({ label, disabled = false, placement = 'bottom', children }: MenuButtonProps) {
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<{ top: number; right: number } | { bottom: number; right: number } | null>(null);
	const btnRef = useRef<HTMLDivElement>(null);
	const popRef = useRef<HTMLDivElement>(null);

	function toggle() {
		if (disabled) return;
		if (!open && btnRef.current) {
			const r = btnRef.current.getBoundingClientRect();
			setPos(menuPosition(r, placement, window.innerHeight, window.innerWidth));
		}
		setOpen((v) => !v);
	}

	useEffect(() => {
		if (disabled) setOpen(false);
	}, [disabled]);

	useEffect(() => {
		if (!open) return;
		const onDoc = (e: PointerEvent) => {
			const t = e.target as Node;
			if (btnRef.current?.contains(t) || popRef.current?.contains(t)) return;
			setOpen(false);
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') setOpen(false);
		};
		document.addEventListener('pointerdown', onDoc);
		document.addEventListener('keydown', onKey);
		return () => {
			document.removeEventListener('pointerdown', onDoc);
			document.removeEventListener('keydown', onKey);
		};
	}, [open]);

	return (
		<div className="menu-anchor" ref={btnRef}>
			<IconButton label={label} disabled={disabled} onClick={(event) => handleMenuToggle(event, toggle)}>
				<MoreVertical size={16} />
			</IconButton>
			{isMenuPopupVisible(open, disabled) &&
				pos &&
				createPortal(
					<div className="menu-pop card" ref={popRef} style={pos} onClick={(event) => event.stopPropagation()}>
						{children(() => setOpen(false))}
					</div>,
					document.body
				)}
		</div>
	);
}
