import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useAddSourceRepo } from '../../api/hooks';
import { Button, Input, Modal, pushToast } from '../../components';

export function repoTitle(url: string, name?: string | null): string {
	if (name) return name;
	try {
		return new URL(url).host;
	} catch {
		return url;
	}
}

export function AddRepoModal({ open, onClose }: { open: boolean; onClose: () => void }) {
	const { t } = useTranslation();
	const addRepo = useAddSourceRepo();
	const [url, setUrl] = useState('');

	// Clear the field each time the modal opens so a half-typed URL from a previous cancel doesn't linger
	useEffect(() => {
		if (open) setUrl('');
	}, [open]);

	const canSubmit = url.trim() !== '';

	async function submit() {
		const trimmed = url.trim();
		if (!trimmed) return;
		try {
			const repo = await addRepo.mutateAsync(trimmed);
			pushToast(t('reposPanel.repoAdded', { name: repoTitle(repo.url, repo.name) }), 'info');
			onClose();
		} catch {
			// client already popped an error toast
		}
	}

	return (
		<Modal
			open={open}
			title={t('reposPanel.addRepo')}
			onClose={onClose}
			footer={
				<>
					<Button onClick={onClose}>{t('common.cancel')}</Button>
					<Button variant="primary" loading={addRepo.isPending} disabled={!canSubmit} onClick={() => void submit()}>
						{t('reposPanel.addRepo')}
					</Button>
				</>
			}
		>
			<div className="opds-form">
				<Input
					type="url"
					placeholder={t('reposPanel.repoUrlPlaceholder')}
					value={url}
					onChange={(e) => setUrl(e.currentTarget.value)}
					autoFocus
				/>
			</div>
		</Modal>
	);
}
