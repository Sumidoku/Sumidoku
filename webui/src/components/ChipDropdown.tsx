import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { Chip } from './Chip';

export interface ChipDropdownProps {
	label: ReactNode;
	/** 启用态高亮 chip */
	active?: boolean;
	/** 拿到 close 以便点选后收起；多选场景不调用即保持菜单打开 */
	children: (close: () => void) => ReactNode;
}

/** chip 下拉菜单：portal 挂到 body + fixed 定位，避免被父级 overflow 裁剪、
 * 被页面内容遮挡；不窄于锚点 chip、夹紧在视口内；Escape 与任意滚动收起 */
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
		// 不窄于锚点宽度，且不溢出视口右缘
		const minWidth = Math.max(rect.width, 160);
		const left = Math.max(8, Math.min(rect.left, window.innerWidth - minWidth - 8));
		setPos({ top: rect.bottom + 4, left, minWidth });
		setOpen(true);
	}

	useEffect(() => {
		if (!open) return;
		const onDown = (e: MouseEvent) => {
			// 菜单 portal 在 body 下、不在 anchor 里，必须把菜单也算 inside——
			// 否则菜单项的 mousedown 先收起菜单，onClick 永远发不出来
			const t = e.target as Node;
			if (!anchorRef.current?.contains(t) && !menuRef.current?.contains(t)) setOpen(false);
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') setOpen(false);
		};
		// 任意滚动收起（fixed 定位不跟随锚点）；菜单自身的滚动除外，否则滚菜单等于关菜单
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
