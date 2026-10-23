import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { MoreVertical } from 'lucide-react';
import { IconButton } from './IconButton';

interface MenuButtonProps {
	label: string;
	/** Entries render through a close callback so an action can dismiss the popover */
	children: (close: () => void) => ReactNode;
}

export function handleMenuToggle(event: { stopPropagation: () => void }, toggle: () => void): void {
	event.stopPropagation();
	toggle();
}

// Anchored dropdown for row-level action menus (chapter rows, list items). The popover goes through a
// portal because row containers commonly clip absolute positioning; same pattern as the manga page's MoreMenu.
export function MenuButton({ label, children }: MenuButtonProps) {
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<{ top: number; right: number } | null>(null);
	const btnRef = useRef<HTMLDivElement>(null);
	const popRef = useRef<HTMLDivElement>(null);

	function toggle() {
		if (!open && btnRef.current) {
			const r = btnRef.current.getBoundingClientRect();
			setPos({ top: r.bottom + 6, right: window.innerWidth - r.right });
		}
		setOpen((v) => !v);
	}

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
			<IconButton label={label} onClick={(event) => handleMenuToggle(event, toggle)}>
				<MoreVertical size={16} />
			</IconButton>
			{open &&
				pos &&
				createPortal(
					<div className="menu-pop card" ref={popRef} style={{ top: pos.top, right: pos.right }} onClick={(event) => event.stopPropagation()}>
						{children(() => setOpen(false))}
					</div>,
					document.body
				)}
		</div>
	);
}
