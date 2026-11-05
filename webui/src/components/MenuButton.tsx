import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { Dispatch, ReactNode, RefObject, SetStateAction } from 'react';
import { createPortal } from 'react-dom';
import { MoreVertical } from 'lucide-react';
import { IconButton } from './IconButton';
import { placeMenuVertical } from './selectModel';

interface MenuButtonProps {
	label: string;
	disabled?: boolean;
	/** Preferred side of the anchor; the menu flips when only the other side has room */
	placement?: 'bottom' | 'top';
	/** Entries render through a close callback so an action can dismiss the popover */
	children: (close: () => void) => ReactNode;
}

/** Fixed-position offsets for a portaled menu, measured against the viewport */
export interface MenuViewportPosition {
	top: number;
	right: number;
	maxHeight: number;
}

export function handleMenuToggle(event: { stopPropagation: () => void }, toggle: () => void): void {
	event.stopPropagation();
	toggle();
}

export function isMenuPopupVisible(open: boolean, disabled: boolean): boolean {
	return open && !disabled;
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

// Runs before paint: only the rendered menu knows its size, and an anchor near an edge of the
// screen (the last row of a list) can leave less room on the preferred side than the menu needs.
export function useMenuInViewport(
	open: boolean,
	anchorRef: RefObject<HTMLElement | null>,
	popRef: RefObject<HTMLElement | null>,
	setPos: Dispatch<SetStateAction<MenuViewportPosition | null>>,
	placement: 'bottom' | 'top' = 'bottom'
): void {
	useLayoutEffect(() => {
		const anchor = anchorRef.current?.getBoundingClientRect();
		const pop = popRef.current?.getBoundingClientRect();
		if (!open || !anchor || !pop) return;
		const width = menuViewportWidth();
		const vertical = placeMenuVertical(anchor, pop.height, window.innerHeight, placement);
		setPos({
			top: vertical.top,
			right: clampMenuRight(width - anchor.right, pop.width, width),
			maxHeight: vertical.maxHeight
		});
	}, [open, anchorRef, popRef, setPos, placement]);
}

// Anchored dropdown for row-level action menus (chapter rows, list items). The popover goes through a
// portal because row containers commonly clip absolute positioning; same pattern as the manga page's MoreMenu.
export function MenuButton({ label, disabled = false, placement = 'bottom', children }: MenuButtonProps) {
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<MenuViewportPosition | null>(null);
	const btnRef = useRef<HTMLDivElement>(null);
	const popRef = useRef<HTMLDivElement>(null);

	function toggle() {
		if (disabled) return;
		setOpen((v) => !v);
	}

	useMenuInViewport(open, btnRef, popRef, setPos, placement);

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
			{/* Renders hidden until measured: the position effect runs before paint, so the menu
				never flashes at the wrong spot */}
			{isMenuPopupVisible(open, disabled) &&
				createPortal(
					<div
						className="menu-pop card"
						ref={popRef}
						style={pos ?? { visibility: 'hidden' }}
						onClick={(event) => event.stopPropagation()}
					>
						{children(() => setOpen(false))}
					</div>,
					document.body
				)}
		</div>
	);
}
