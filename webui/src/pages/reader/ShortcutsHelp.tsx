import { useTranslation } from 'react-i18next';
import { Modal } from '../../components';

const GROUPS: { titleKey: string; rows: [string, string][] }[] = [
	{
		titleKey: 'reader.shortcuts.navigation',
		rows: [
			['← / →', 'reader.shortcuts.turnPage'],
			['↑ / ↓', 'reader.shortcuts.scrollVertical'],
			['Space / Shift+Space', 'reader.shortcuts.pageForwardBack'],
			['PgUp / PgDn', 'reader.shortcuts.pageForwardBack'],
			['Home / End', 'reader.shortcuts.firstLastPage']
		]
	},
	{
		titleKey: 'reader.shortcuts.reading',
		rows: [
			['C', 'reader.shortcuts.cycleScale'],
			['F', 'reader.shortcuts.toggleFullscreen']
		]
	},
	{
		titleKey: 'reader.shortcuts.menus',
		rows: [
			['M', 'reader.shortcuts.toggleToolbars'],
			['H', 'reader.shortcuts.thisHelp'],
			['Esc', 'reader.shortcuts.closeExit']
		]
	}
];

export function ShortcutsHelp({ open, onClose, portalTarget }: { open: boolean; onClose: () => void; portalTarget?: Element }) {
	const { t } = useTranslation();
	return (
		<Modal open={open} title={t('reader.shortcuts.title')} onClose={onClose} portalTarget={portalTarget} panelClassName="modal__panel--wide reader__shortcuts">
			<div className="reader__shortcut-groups">
				{GROUPS.map((group) => (
					<section key={group.titleKey}>
						<h3 className="reader__shortcut-title">{t(group.titleKey)}</h3>
						<div className="reader__shortcut-list">
							{group.rows.map(([keys, descriptionKey]) => (
								<div className="reader__shortcut-row" key={keys}>
									<span>{t(descriptionKey)}</span>
									<kbd>{keys}</kbd>
								</div>
							))}
						</div>
					</section>
				))}
			</div>
		</Modal>
	);
}
