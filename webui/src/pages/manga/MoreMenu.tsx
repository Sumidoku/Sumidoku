import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { MoreVertical } from 'lucide-react';
import { IconButton } from '../../components';

// 弹层走 portal：hero 容器 overflow:hidden，absolute 定位会被裁掉
export function MoreMenu({ children }: { children: (close: () => void) => ReactNode }) {
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
		<div className="manga-menu" ref={btnRef}>
			<IconButton label="更多操作" onClick={toggle}>
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
