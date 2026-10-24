import { useTranslation } from 'react-i18next';
import { Trash2 } from 'lucide-react';
import { Button } from './Button';
import { IconButton } from './IconButton';
import { Input } from './Input';

/** Controlled editor for a local instance's root directories: one input per path plus add/remove rows */
export function LocalDirsEditor({ dirs, onChange }: { dirs: string[]; onChange: (dirs: string[]) => void }) {
	const { t } = useTranslation();
	return (
		<>
			{dirs.length > 0 && (
				<div className="settings-kv">
					{dirs.map((dir, i) => (
						<div key={i} className="settings-kv__row">
							<Input
								placeholder={t('sourceSettings.localDirPlaceholder')}
								value={dir}
								onChange={(e) => {
									// currentTarget is already null by the time the deferred updater runs, so take the value first
									const v = e.currentTarget.value;
									onChange(dirs.map((d, j) => (j === i ? v : d)));
								}}
							/>
							<IconButton
								label={t('common.delete')}
								variant="danger"
								onClick={() => onChange(dirs.filter((_, j) => j !== i))}
							>
								<Trash2 size={16} />
							</IconButton>
						</div>
					))}
				</div>
			)}
			<div className="settings-kv__add">
				<Button onClick={() => onChange([...dirs, ''])}>{t('sourceSettings.addLocalDir')}</Button>
			</div>
		</>
	);
}
