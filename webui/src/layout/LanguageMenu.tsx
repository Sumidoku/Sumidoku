import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { Check, Languages } from 'lucide-react';
import { IconButton, useMenuInViewport } from '../components';
import type { MenuViewportPosition } from '../components';
import { LANGUAGE_OPTIONS, setLanguage } from '../i18n';

// Same portal + fixed-position pattern as MenuButton/MoreMenu: the sidebar foot clips
// absolutely positioned menus, and fixed coordinates stop following the anchor on scroll
export function LanguageMenu() {
	const { t, i18n } = useTranslation();
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<MenuViewportPosition | null>(null);
	const btnRef = useRef<HTMLDivElement>(null);
	const popRef = useRef<HTMLDivElement>(null);

	// The sidebar foot hugs the viewport bottom edge; the measured flip keeps the menu inside
	// the screen no matter how many languages the list grows to
	useMenuInViewport(open, btnRef, popRef, setPos);

	useEffect(() => {
		if (!open) return;
		const onDown = (e: MouseEvent) => {
			const target = e.target as Node;
			if (!btnRef.current?.contains(target) && !popRef.current?.contains(target)) setOpen(false);
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') setOpen(false);
		};
		const onScroll = (e: Event) => {
			if (popRef.current?.contains(e.target as Node)) return;
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
		<div className="menu-anchor" ref={btnRef}>
			<IconButton label={t('settings.language')} onClick={() => setOpen((v) => !v)} aria-expanded={open}>
				<Languages size={17} />
			</IconButton>
			{/* Renders hidden until measured: the position effect runs before paint */}
			{open &&
				createPortal(
					<div className="menu-pop card" ref={popRef} style={pos ?? { visibility: 'hidden' }}>
						{LANGUAGE_OPTIONS.map((option) => (
							<button
								key={option.value}
								type="button"
								className="menu-row"
								aria-current={i18n.language === option.value}
								onClick={() => {
									setLanguage(option.value);
									setOpen(false);
								}}
							>
								<span>{option.label}</span>
								{i18n.language === option.value && <Check size={14} aria-hidden="true" />}
							</button>
						))}
					</div>,
					document.body
				)}
		</div>
	);
}
