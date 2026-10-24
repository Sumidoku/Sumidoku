import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { NavLink, useLocation } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { ChevronRight, Database, Download, Info, Library, PanelLeftClose, PanelLeftOpen, Settings as SettingsIcon } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { IconButton } from '../components';
import { NAV } from './navigation';

interface NavItemData {
	to: string;
	labelKey: string;
	icon: LucideIcon;
}

interface DesktopSidebarProps {
	collapsed: boolean;
	onToggle: () => void;
	children: ReactNode;
}

const LIBRARY_NAV = NAV.filter((item) => ['/library', '/browse', '/search', '/history'].includes(item.to));
const ACTIVITY_NAV = NAV.filter((item) => ['/downloads', '/updates'].includes(item.to));
const SETTINGS_NAV: readonly NavItemData[] = [
	{ to: '/settings', labelKey: 'settings.groupGeneral', icon: SettingsIcon },
	{ to: '/settings?section=library', labelKey: 'settings.groupLibrary', icon: Library },
	{ to: '/settings?section=downloads', labelKey: 'settings.groupDownloads', icon: Download },
	{ to: '/settings?section=backups', labelKey: 'settings.groupBackups', icon: Database },
	{ to: '/settings?section=system', labelKey: 'settings.groupSystem', icon: Info }
];

function NavItem({ item, collapsed }: { item: NavItemData; collapsed: boolean }) {
	const { t } = useTranslation();
	const Icon = item.icon;
	return (
		<NavLink
			to={item.to}
			className={({ isActive }) => isActive ? 'side-nav__item side-nav__item--active' : 'side-nav__item'}
			aria-label={collapsed ? t(item.labelKey) : undefined}
			title={collapsed ? t(item.labelKey) : undefined}
		>
			<Icon size={18} aria-hidden="true" />
			<span className="side-nav__label" hidden={collapsed}>{t(item.labelKey)}</span>
		</NavLink>
	);
}

function NavGroup({
	labelKey,
	icon: Icon,
	match,
	items,
	collapsed
}: {
	labelKey: string;
	icon: LucideIcon;
	match: readonly string[];
	items: readonly NavItemData[];
	collapsed: boolean;
}) {
	const { t } = useTranslation();
	const { pathname } = useLocation();
	const containsActive = match.some((prefix) => pathname.startsWith(prefix));
	const [open, setOpen] = useState(containsActive);
	useEffect(() => {
		if (containsActive) setOpen(true);
	}, [containsActive]);
	const id = `side-nav-group-${labelKey}`;
	return (
		<div>
			<button
				type="button"
				className="side-nav__item side-nav__group-trigger"
				data-nav-group={labelKey}
				aria-expanded={open}
				aria-controls={id}
				aria-label={collapsed ? t(labelKey) : undefined}
				title={collapsed ? t(labelKey) : undefined}
				onClick={() => setOpen((value) => !value)}
			>
				<Icon size={18} aria-hidden="true" />
				<span className="side-nav__label" hidden={collapsed}>{t(labelKey)}</span>
				<ChevronRight size={14} className={open ? 'side-nav__chevron side-nav__chevron--open' : 'side-nav__chevron'} />
			</button>
			<div id={id} className="side-nav__children" hidden={!open}>
				{items.map((item) => <NavItem key={item.to} item={item} collapsed={collapsed} />)}
			</div>
		</div>
	);
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
				<NavGroup
					labelKey="common.nav.library"
					icon={Library}
					match={['/library', '/browse', '/search', '/history', '/manga', '/reader']}
					items={LIBRARY_NAV}
					collapsed={collapsed}
				/>
				<NavGroup
					labelKey="common.nav.downloads"
					icon={Download}
					match={['/downloads', '/updates']}
					items={ACTIVITY_NAV}
					collapsed={collapsed}
				/>
				<NavGroup
					labelKey="common.nav.settings"
					icon={SettingsIcon}
					match={['/settings']}
					items={SETTINGS_NAV}
					collapsed={collapsed}
				/>
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
