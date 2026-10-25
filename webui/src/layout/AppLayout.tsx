import { useEffect, useState } from 'react';
import { NavLink, Outlet, ScrollRestoration } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { Moon, Sun } from 'lucide-react';
import { IconButton } from '../components';
import { readSidebarCollapsed, writeSidebarCollapsed } from '../lib/sidebarPrefs';
import { useResolvedTheme, useTheme } from '../lib/theme';
import { subscribeTaskEvents, useWhoami } from '../api/hooks';
import { sse } from '../api/sse';
import { authAllowsEvents } from '../lib/auth';
import { DesktopSidebar } from './DesktopSidebar';
import { LanguageMenu } from './LanguageMenu';
import { NAV } from './navigation';

function ThemeToggle() {
	const { t } = useTranslation();
	const [, setTheme] = useTheme();
	const resolved = useResolvedTheme();
	const next = resolved === 'dark' ? 'light' : 'dark';
	return (
		<IconButton
			label={next === 'light' ? t('layout.toLight') : t('layout.toDark')}
			onClick={() => setTheme(next)}
		>
			{resolved === 'dark' ? <Sun size={17} /> : <Moon size={17} />}
		</IconButton>
	);
}

/** The full-screen reader does not come through here (its route hangs outside AppLayout) */
export function AppLayout() {
	const { t } = useTranslation();
	const [sidebarCollapsed, setSidebarCollapsed] = useState(readSidebarCollapsed);
	const auth = useWhoami();
	const qc = useQueryClient();
	useEffect(() => {
		writeSidebarCollapsed(sidebarCollapsed);
	}, [sidebarCollapsed]);
	useEffect(() => {
		if (authAllowsEvents(auth.data)) sse.start();
		else sse.stop();
	}, [auth.data]);
	// Task-event invalidation lives here (one owner for the whole app); pages only read the query cache
	useEffect(() => subscribeTaskEvents(qc), [qc]);
	useEffect(() => () => sse.stop(), []);
	return (
		<div className={sidebarCollapsed ? 'app app--sidebar-collapsed' : 'app'}>
			<DesktopSidebar collapsed={sidebarCollapsed} onToggle={() => setSidebarCollapsed((current) => !current)}>
				<div className="app__shell-actions">
					<LanguageMenu />
					<ThemeToggle />
				</div>
			</DesktopSidebar>

			<header className="app__topbar">
				<NavLink to="/library" className="app__logo app__logo--sm">
					<img className="app__logo-mark" src="/icon.svg" alt="" />
					<span className="app__logo-text">Sumidoku</span>
				</NavLink>
				<div className="app__shell-actions">
					<LanguageMenu />
					<ThemeToggle />
				</div>
			</header>

			<main className="app__content">
				<Outlet />
			</main>
			{/* New navigation scrolls to top, forward/back restores each page's own position; the reader lives outside the layout with its own scroll management, unaffected */}
			<ScrollRestoration />

			<nav className="app__tabbar">
				{NAV.map((item) => (
					<NavLink
						key={item.to}
						to={item.to}
						className={({ isActive }) =>
							isActive ? 'tabbar__item tabbar__item--active' : 'tabbar__item'
						}
					>
						<item.icon size={20} />
						<span className="tabbar__label">{t(item.labelKey)}</span>
					</NavLink>
				))}
			</nav>
		</div>
	);
}
