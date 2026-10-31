import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { MoreVertical } from 'lucide-react';
import { IconButton, menuViewportWidth, useMenuInViewport } from '../../components';

// The popover goes through a portal: the hero container is overflow:hidden, so absolute positioning would be clipped
export function MoreMenu({ children }: { children: (close: () => void) => ReactNode }) {
	const { t } = useTranslation();
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<{ top: number; right: number } | null>(null);
	const btnRef = useRef<HTMLDivElement>(null);
	const popRef = useRef<HTMLDivElement>(null);

	function toggle() {
		if (!open && btnRef.current) {
			const r = btnRef.current.getBoundingClientRect();
			setPos({ top: r.bottom + 6, right: menuViewportWidth() - r.right });
		}
		setOpen((v) => !v);
	}

	useMenuInViewport(open, popRef, setPos);

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
			{open &&
				pos &&
				createPortal(
					<div
						className="manga-menu__pop card"
						ref={popRef}
						style={{ top: pos.top, right: pos.right }}
					>
						{children(() => setOpen(false))}
					</div>,
					document.body
				)}
		</div>
	);
}
