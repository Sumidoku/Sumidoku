import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { menuViewportWidth } from './MenuButton';
import { placeMenu } from './selectModel';

const MIN_WIDTH = 160;

/** Open state and placement for a menu portaled to body with fixed positioning, so it is neither
 * clipped by a parent's overflow nor covered by page content. Outside presses, Escape, resizes and
 * any scroll dismiss it. */
export function useAnchoredMenu<A extends HTMLElement>() {
	const [open, setOpen] = useState(false);
	const [style, setStyle] = useState<CSSProperties>({ visibility: 'hidden' });
	const anchorRef = useRef<A>(null);
	const menuRef = useRef<HTMLDivElement>(null);

	const close = useCallback(() => setOpen(false), []);
	const show = useCallback(() => {
		const rect = anchorRef.current?.getBoundingClientRect();
		if (!rect) return;
		// Measured hidden first: the menu's real size decides where it goes
		setStyle({ top: 0, left: 0, minWidth: Math.max(rect.width, MIN_WIDTH), visibility: 'hidden' });
		setOpen(true);
	}, []);

	useLayoutEffect(() => {
		const anchor = anchorRef.current?.getBoundingClientRect();
		const menu = menuRef.current?.getBoundingClientRect();
		if (!open || !anchor || !menu) return;
		const placement = placeMenu(anchor, menu, { width: menuViewportWidth(), height: window.innerHeight });
		setStyle((current) => ({ ...current, ...placement, visibility: 'visible' }));
	}, [open]);

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
		// Fixed positioning does not follow the anchor; scrolling inside the menu itself is exempt, else scrolling the menu would close it
		const onScroll = (e: Event) => {
			if (menuRef.current?.contains(e.target as Node)) return;
			setOpen(false);
		};
		document.addEventListener('mousedown', onDown);
		document.addEventListener('keydown', onKey);
		document.addEventListener('scroll', onScroll, true);
		window.addEventListener('resize', close);
		return () => {
			document.removeEventListener('mousedown', onDown);
			document.removeEventListener('keydown', onKey);
			document.removeEventListener('scroll', onScroll, true);
			window.removeEventListener('resize', close);
		};
	}, [open, close]);

	return { open, style, anchorRef, menuRef, show, close };
}
