import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { Check, Languages } from 'lucide-react';
import { IconButton } from '../components';
import { LANGUAGE_OPTIONS, setLanguage } from '../i18n';

// Same portal + fixed-position pattern as ChipDropdown/MenuButton: the sidebar foot clips
// absolutely positioned menus, and fixed coordinates stop following the anchor on scroll
export function LanguageMenu() {
	const { t, i18n } = useTranslation();
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<{ left: number; top?: number; bottom?: number } | null>(null);
	const anchorRef = useRef<HTMLDivElement>(null);
	const menuRef = useRef<HTMLDivElement>(null);

	function toggle() {
		if (open) {
			setOpen(false);
			return;
		}
		const rect = anchorRef.current?.getBoundingClientRect();
		if (!rect) return;
		const left = Math.max(8, Math.min(rect.left, window.innerWidth - 188));
		// The sidebar foot hugs the viewport bottom edge; flip the menu upward when it would overflow
		if (rect.bottom + 396 > window.innerHeight) {
			setPos({ left, bottom: window.innerHeight - rect.top + 6 });
		} else {
			setPos({ left, top: rect.bottom + 6 });
		}
		setOpen(true);
	}

	useEffect(() => {
		if (!open) return;
		const onDown = (e: MouseEvent) => {
			const target = e.target as Node;
			if (!anchorRef.current?.contains(target) && !menuRef.current?.contains(target)) setOpen(false);
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') setOpen(false);
		};
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
		<div className="menu-anchor" ref={anchorRef}>
			<IconButton label={t('settings.language')} onClick={toggle} aria-expanded={open}>
				<Languages size={17} />
			</IconButton>
			{open &&
				pos &&
				createPortal(
					<div
						className="menu-pop card"
						ref={menuRef}
						style={{ ...pos, maxHeight: window.innerHeight - 16, overflowY: 'auto' }}
					>
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
