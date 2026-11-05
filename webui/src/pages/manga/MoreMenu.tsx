import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { MoreVertical } from 'lucide-react';
import { IconButton, useMenuInViewport } from '../../components';
import type { MenuViewportPosition } from '../../components';

// The popover goes through a portal: the hero container is overflow:hidden, so absolute positioning would be clipped
export function MoreMenu({ children }: { children: (close: () => void) => ReactNode }) {
	const { t } = useTranslation();
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<MenuViewportPosition | null>(null);
	const btnRef = useRef<HTMLDivElement>(null);
	const popRef = useRef<HTMLDivElement>(null);

	function toggle() {
		setOpen((v) => !v);
	}

	useMenuInViewport(open, btnRef, popRef, setPos);

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
		<div className="manga-menu" ref={btnRef}>
			<IconButton label={t('moreMenu.label')} onClick={toggle}>
				<MoreVertical size={17} />
			</IconButton>
			{/* Hidden until measured, same as MenuButton: the position effect runs before paint */}
			{open &&
				createPortal(
					<div className="manga-menu__pop card" ref={popRef} style={pos ?? { visibility: 'hidden' }}>
						{children(() => setOpen(false))}
					</div>,
					document.body
				)}
		</div>
	);
}
