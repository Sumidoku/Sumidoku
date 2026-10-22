import type { ReactNode } from 'react';
import { NavLink } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react';
import { IconButton } from '../components';
import { NAV } from './navigation';

interface DesktopSidebarProps {
	collapsed: boolean;
	onToggle: () => void;
	children: ReactNode;
}

export function DesktopSidebar({ collapsed, onToggle, children }: DesktopSidebarProps) {
	const { t } = useTranslation();
	return (
		<aside id="desktop-sidebar" className={collapsed ? 'app__sidebar app__sidebar--collapsed' : 'app__sidebar'}>
			<NavLink
				to="/library"
				className="app__logo"
				aria-label={collapsed ? 'Sumidoku' : undefined}
				title={collapsed ? 'Sumidoku' : undefined}
			>
				<img className="app__logo-mark" src="/icon.svg" alt="" />
				<span className="app__logo-text" hidden={collapsed}>Sumidoku</span>
			</NavLink>
			<nav className="side-nav" aria-label={t('layout.primaryNav')}>
				{NAV.map((item) => (
					<NavLink
						key={item.to}
						to={item.to}
						className={({ isActive }) =>
							isActive ? 'side-nav__item side-nav__item--active' : 'side-nav__item'
						}
						aria-label={collapsed ? t(item.labelKey) : undefined}
						title={collapsed ? t(item.labelKey) : undefined}
					>
						<item.icon size={18} aria-hidden="true" />
						<span className="side-nav__label" hidden={collapsed}>{t(item.labelKey)}</span>
					</NavLink>
				))}
			</nav>
			<div className="app__sidebar-foot">
				<IconButton
					className="app__sidebar-toggle"
					label={collapsed ? t('layout.expandSidebar') : t('layout.collapseSidebar')}
					onClick={onToggle}
					aria-expanded={!collapsed}
					aria-controls="desktop-sidebar"
				>
					{collapsed ? <PanelLeftOpen size={17} /> : <PanelLeftClose size={17} />}
				</IconButton>
				{children}
			</div>
		</aside>
	);
}
