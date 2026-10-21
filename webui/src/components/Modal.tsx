import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { X } from 'lucide-react';
import { IconButton } from './IconButton';

export interface ModalProps {
	open: boolean;
	title: string;
	onClose: () => void;
	children: ReactNode;
	footer?: ReactNode;
	/** Class name appended to the panel, e.g. to widen it */
	panelClassName?: string;
}

const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

export function Modal({ open, title, onClose, children, footer, panelClassName }: ModalProps) {
	const { t } = useTranslation();
	const panelRef = useRef<HTMLDivElement>(null);

	useEffect(() => {
		if (!open) return;
		panelRef.current?.focus();
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') {
				onClose();
				return;
			}
			if (e.key !== 'Tab' || !panelRef.current) return;
			const els = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
				(el) => !el.hasAttribute('disabled')
			);
			if (els.length === 0) return;
			const first = els[0];
			const last = els[els.length - 1];
			if (e.shiftKey && document.activeElement === first) {
				last.focus();
				e.preventDefault();
			} else if (!e.shiftKey && document.activeElement === last) {
				first.focus();
				e.preventDefault();
			}
		};
		document.addEventListener('keydown', onKey);
		return () => document.removeEventListener('keydown', onKey);
	}, [open, onClose]);

	if (!open) return null;

	return createPortal(
		<div className="modal__mask" onClick={onClose}>
			<div
				className={panelClassName ? `modal__panel ${panelClassName}` : 'modal__panel'}
				role="dialog"
				aria-modal="true"
				aria-label={title}
				tabIndex={-1}
				ref={panelRef}
				onClick={(e) => e.stopPropagation()}
			>
				<div className="modal__head">
					<h3>{title}</h3>
					<IconButton label={t('modal.close')} size="sm" onClick={onClose}>
						<X size={16} />
					</IconButton>
				</div>
				<div className="modal__body">{children}</div>
				{footer && <div className="modal__footer">{footer}</div>}
			</div>
		</div>,
		document.body
	);
}
