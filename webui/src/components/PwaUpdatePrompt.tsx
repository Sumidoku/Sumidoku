import { useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { pwaUpdateController } from '../pwa/updateController';
import { Button } from './Button';

export function PwaUpdatePrompt() {
	const { t } = useTranslation();
	const visible = useSyncExternalStore(
		pwaUpdateController.subscribe,
		pwaUpdateController.getSnapshot,
		pwaUpdateController.getSnapshot
	);
	if (!visible) return null;

	return (
		<div className="pwa-update" role="region" aria-label={t('pwa.updateLabel')}>
			<div className="toast toast--info pwa-update__toast">
				<span className="pwa-update__message" role="status">
					{t('pwa.updateAvailable')}
				</span>
				<div className="pwa-update__actions">
					<Button variant="primary" size="sm" onClick={() => void pwaUpdateController.reload()}>
						{t('pwa.reload')}
					</Button>
					<Button size="sm" onClick={pwaUpdateController.dismiss}>
						{t('pwa.later')}
					</Button>
				</div>
			</div>
		</div>
	);
}
