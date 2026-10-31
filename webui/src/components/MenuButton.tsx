import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { Dispatch, ReactNode, RefObject, SetStateAction } from 'react';
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

const MENU_EDGE = 8;

/** A fixed element's `right` is measured from inside a classic scrollbar, which window.innerWidth includes */
export function menuViewportWidth(): number {
	return document.documentElement.clientWidth;
}

/** Right-aligned to its anchor until the menu would cross the left edge of the viewport */
export function clampMenuRight(right: number, menuWidth: number, viewportWidth: number): number {
	return Math.max(MENU_EDGE, Math.min(right, viewportWidth - menuWidth - MENU_EDGE));
}

// Runs before paint: only the rendered menu knows its width, and an anchor near the middle of a phone
// screen (the manga hero's actions) leaves less room to its left than the menu needs.
export function useMenuInViewport<P extends { right: number }>(
	open: boolean,
	popRef: RefObject<HTMLElement | null>,
	setPos: Dispatch<SetStateAction<P | null>>
): void {
	useLayoutEffect(() => {
		const pop = popRef.current;
		if (!open || !pop) return;
		const width = pop.getBoundingClientRect().width;
		setPos((pos) => pos && { ...pos, right: clampMenuRight(pos.right, width, menuViewportWidth()) });
	}, [open, popRef, setPos]);
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
			setPos(menuPosition(r, placement, window.innerHeight, menuViewportWidth()));
		}
		setOpen((v) => !v);
	}

	useMenuInViewport(open, popRef, setPos);

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
