import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { mergeServerSettings, useCategories, useSaveServerSettings, useServerSettings } from '../../api/hooks';
import { Button, Modal, Skeleton, Switch } from '../../components';

export function ExcludedCategoriesSetting({
	kind,
	title,
	description
}: {
	kind: 'update' | 'download';
	title: string;
	description: string;
}) {
	const { t } = useTranslation();
	const settings = useServerSettings();
	const save = useSaveServerSettings();
	const categories = useCategories();
	const [choosing, setChoosing] = useState(false);
	const excluded = (kind === 'update'
		? settings.data?.library.excludedUpdateCategories
		: settings.data?.library.excludedDownloadCategories) ?? [];

	function toggle(id: number, on: boolean) {
		const changed = on ? [...excluded, id] : excluded.filter((value) => value !== id);
		save.mutate(
			mergeServerSettings(
				settings.data,
				kind === 'update'
					? { library: { excludedUpdateCategories: changed } }
					: { library: { excludedDownloadCategories: changed } }
			)
		);
	}

	return (
		<>
			<div className="settings-row settings-row--top">
				<div className="settings-row__stack">
					<span className="settings-row__label">{title}</span>
					<p className="settings-cat__hint">{description}</p>
				</div>
				<div className="settings-excluded-cats">
					{categories.isPending ? (
						<Skeleton height={40} width={120} />
					) : (categories.data ?? []).length === 0 ? (
						<p className="faint">{t('settings.noCategories')}</p>
					) : (
						<>
							<span className="settings-excluded-cats__summary">
								{excluded.length === 0
									? t('settings.noExcludedCategories')
									: t('settings.excludedCategoriesCount', { count: excluded.length })}
							</span>
							<Button size="sm" disabled={save.isPending} onClick={() => setChoosing(true)}>
								{t('settings.chooseExcludedCategories')}
							</Button>
						</>
					)}
				</div>
			</div>

			<Modal
				open={choosing}
				title={title}
				onClose={() => setChoosing(false)}
				footer={<Button onClick={() => setChoosing(false)}>{t('modal.close')}</Button>}
			>
				<div className="settings-excluded-cats__modal">
					{(categories.data ?? []).length === 0 ? (
						<p className="faint">{t('settings.noCategories')}</p>
					) : (
						(categories.data ?? []).map((category) => (
							<label key={category.id} className="settings-cat-check">
								<Switch
									checked={excluded.includes(category.id)}
									disabled={save.isPending}
									onChange={(on) => toggle(category.id, on)}
									label={category.name}
								/>
								<span className="settings-cat-check__name">{category.name}</span>
							</label>
						))
					)}
				</div>
			</Modal>
		</>
	);
}
