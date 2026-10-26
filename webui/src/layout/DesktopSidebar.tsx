import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, NavLink, useLocation } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { Activity, ChevronDown, ChevronRight, Database, Download, Info, Library, Link2, PanelLeftClose, PanelLeftOpen, Settings as SettingsIcon, SlidersHorizontal } from 'lucide-react';
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

const PRIMARY_NAV = NAV.filter((item) => ['/library', '/browse', '/updates'].includes(item.to));
const ACTIVITY_NAV = NAV.filter((item) => ['/history', '/downloads'].includes(item.to));
const SETTINGS_NAV: readonly NavItemData[] = [
	{ to: '/settings', labelKey: 'settings.groupGeneral', icon: SlidersHorizontal },
	{ to: '/settings?section=library', labelKey: 'settings.groupLibrary', icon: Library },
	{ to: '/settings?section=trackers', labelKey: 'settings.groupTrackers', icon: Link2 },
	{ to: '/settings?section=downloads', labelKey: 'settings.groupDownloads', icon: Download },
	{ to: '/settings?section=backups', labelKey: 'settings.groupBackups', icon: Database },
	{ to: '/settings?section=system', labelKey: 'settings.groupSystem', icon: Info }
];

function NavItem({ item, collapsed }: { item: NavItemData; collapsed: boolean }) {
	const { t } = useTranslation();
	const { pathname, search } = useLocation();
	const Icon = item.icon;
	const [path, query = ''] = item.to.split('?');
	const currentSection = new URLSearchParams(search).get('section') ?? 'general';
	const itemSection = new URLSearchParams(query).get('section') ?? 'general';
	const pathMatches = pathname === path || pathname.startsWith(`${path}/`);
	const isActive = pathMatches && (path !== '/settings' || currentSection === itemSection);
	return (
		<Link
			to={item.to}
			className={isActive ? 'side-nav__item side-nav__item--active' : 'side-nav__item'}
			aria-current={isActive ? 'page' : undefined}
			aria-label={collapsed ? t(item.labelKey) : undefined}
			title={collapsed ? t(item.labelKey) : undefined}
		>
			<Icon size={18} aria-hidden="true" />
			<span className="side-nav__label" hidden={collapsed}>{t(item.labelKey)}</span>
		</Link>
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
				{collapsed && open
					? <ChevronDown size={18} aria-hidden="true" className="side-nav__group-open-mark" />
					: <Icon size={18} aria-hidden="true" />}
				<span className="side-nav__label" hidden={collapsed}>{t(labelKey)}</span>
				<ChevronRight size={14} className={open ? 'side-nav__chevron side-nav__chevron--open' : 'side-nav__chevron'} />
			</button>
			<div id={id} className="side-nav__children" hidden={!open} style={{ display: open ? undefined : 'none' }}>
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
				{PRIMARY_NAV.map((item) => <NavItem key={item.to} item={item} collapsed={collapsed} />)}
				<NavGroup
					labelKey="common.nav.activity"
					icon={Activity}
					match={['/history', '/downloads']}
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
