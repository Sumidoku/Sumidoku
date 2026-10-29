import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

export const SETTINGS_GROUPS = [
	['general', 'settings.groupGeneral'],
	['reader', 'settings.groupReader'],
	['library', 'settings.groupLibrary'],
	['trackers', 'settings.groupTrackers'],
	['downloads', 'settings.groupDownloads'],
	['backups', 'settings.groupBackups'],
	['system', 'settings.groupSystem']
] as const;

export type SettingsGroupKey = (typeof SETTINGS_GROUPS)[number][0];

interface SettingsLayoutProps {
	current: SettingsGroupKey;
	onSelect: (key: SettingsGroupKey) => void;
	panels: Record<SettingsGroupKey, ReactNode>;
}

export function SettingsLayout({ current, onSelect, panels }: SettingsLayoutProps) {
	const { t } = useTranslation();
	const active = SETTINGS_GROUPS.some(([key]) => key === current) ? current : 'general';
	return (
		<div className="settings-layout">
			<nav className="settings-nav" aria-label={t('settings.groupNav')}>
				{SETTINGS_GROUPS.map(([key, labelKey]) => (
					<button
						type="button"
						key={key}
						data-settings-group={key}
						className={active === key ? 'settings-nav__item settings-nav__item--active' : 'settings-nav__item'}
						aria-current={active === key ? 'page' : undefined}
						onClick={() => onSelect(key)}
					>
						{t(labelKey)}
					</button>
				))}
			</nav>
			<div className="settings-panel">
				{SETTINGS_GROUPS.map(([key]) => (
					<div key={key} data-settings-panel={key} hidden={active !== key}>
						{panels[key]}
					</div>
				))}
			</div>
		</div>
	);
}
