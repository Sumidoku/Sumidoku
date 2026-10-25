import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { Category } from '../../api/types';
import { Button, Modal, Switch } from '../../components';

export interface ChangeCategoriesDialogProps {
	/** Selected manga count, shown so the replace scope is explicit */
	count: number;
	categories: Category[];
	busy: boolean;
	onCancel: () => void;
	onApply: (categoryIds: number[]) => void;
}

/** The modal's content, kept portal-free so SSR tests can render it directly; ChangeCategoriesModal supplies the frame */
export function ChangeCategoriesDialog({ count, categories, busy, onCancel, onApply }: ChangeCategoriesDialogProps) {
	const { t } = useTranslation();
	// The selected entries may hold different sets, so the dialog starts blank instead of prechecking a misleading mix
	const [catSel, setCatSel] = useState<Set<number>>(new Set());

	function toggleCat(id: number) {
		setCatSel((prev) => {
			const next = new Set(prev);
			if (next.has(id)) next.delete(id);
			else next.add(id);
			return next;
		});
	}

	return (
		<>
			<p className="muted">{t('library.batchSetCategoriesHint', { count })}</p>
			{categories.length === 0 ? (
				<p className="muted">{t('library.noCategories')}</p>
			) : (
				<div className="library-cats">
					{categories.map((c) => (
						<label key={c.id} className="library-cats__item">
							<span>{c.name}</span>
							<Switch checked={catSel.has(c.id)} onChange={() => toggleCat(c.id)} label={c.name} />
						</label>
					))}
				</div>
			)}
			<div className="library-dialog__actions">
				<Button onClick={onCancel}>{t('common.cancel')}</Button>
				<Button variant="primary" loading={busy} onClick={() => onApply([...catSel])}>
					{t('common.save')}
				</Button>
			</div>
		</>
	);
}

export interface ChangeCategoriesModalProps {
	open: boolean;
	count: number;
	categories: Category[];
	busy: boolean;
	onClose: () => void;
	onApply: (categoryIds: number[]) => void;
}

export function ChangeCategoriesModal({ open, count, categories, busy, onClose, onApply }: ChangeCategoriesModalProps) {
	const { t } = useTranslation();
	return (
		<Modal open={open} title={t('library.batchSetCategories')} onClose={onClose}>
			{/* Remounted per open, so a closed dialog never carries a stale selection into the next batch */}
			{open && (
				<ChangeCategoriesDialog
					count={count}
					categories={categories}
					busy={busy}
					onCancel={onClose}
					onApply={onApply}
				/>
			)}
		</Modal>
	);
}

export interface RemoveFromLibraryDialogProps {
	count: number;
	busy: boolean;
	onCancel: () => void;
	onConfirm: () => void;
}

export function RemoveFromLibraryDialog({ count, busy, onCancel, onConfirm }: RemoveFromLibraryDialogProps) {
	const { t } = useTranslation();
	return (
		<>
			<p>{t('library.batchRemoveHint', { count })}</p>
			<div className="library-dialog__actions">
				<Button onClick={onCancel}>{t('common.cancel')}</Button>
				<Button variant="danger" loading={busy} onClick={onConfirm}>
					{t('library.batchRemoveConfirm')}
				</Button>
			</div>
		</>
	);
}

export interface RemoveFromLibraryModalProps {
	open: boolean;
	count: number;
	busy: boolean;
	onClose: () => void;
	onConfirm: () => void;
}

export function RemoveFromLibraryModal({ open, count, busy, onClose, onConfirm }: RemoveFromLibraryModalProps) {
	const { t } = useTranslation();
	return (
		<Modal open={open} title={t('library.batchRemove')} onClose={onClose}>
			{open && (
				<RemoveFromLibraryDialog count={count} busy={busy} onCancel={onClose} onConfirm={onConfirm} />
			)}
		</Modal>
	);
}
