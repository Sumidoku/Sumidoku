import { NavLink, Outlet } from 'react-router-dom';
import {
	Clock,
	Compass,
	Download,
	Library,
	Moon,
	RefreshCw,
	Settings,
	Sun
} from 'lucide-react';
import { IconButton } from '../components';
import { useResolvedTheme, useTheme } from '../lib/theme';

const NAV = [
	{ to: '/library', label: '书库', icon: Library },
	{ to: '/browse', label: '浏览', icon: Compass },
	{ to: '/history', label: '历史', icon: Clock },
	{ to: '/downloads', label: '下载', icon: Download },
	{ to: '/updates', label: '更新', icon: RefreshCw },
	{ to: '/settings', label: '设置', icon: Settings }
] as const;

function ThemeToggle() {
	const [, setTheme] = useTheme();
	const resolved = useResolvedTheme();
	const next = resolved === 'dark' ? 'light' : 'dark';
	return (
		<IconButton
			label={next === 'light' ? '切到浅色' : '切到深色'}
			onClick={() => setTheme(next)}
		>
			{resolved === 'dark' ? <Sun size={17} /> : <Moon size={17} />}
		</IconButton>
	);
}

/** 阅读器全屏不进这里（路由层挂在 AppLayout 之外） */
export function AppLayout() {
	return (
		<div className="app">
			<aside className="app__sidebar">
				<NavLink to="/library" className="app__logo">
					<img className="app__logo-mark" src="/icon.svg" alt="" />
					<span className="app__logo-text">Sumidoku</span>
				</NavLink>
				<nav className="side-nav">
					{NAV.map((item) => (
						<NavLink
							key={item.to}
							to={item.to}
							className={({ isActive }) =>
								isActive ? 'side-nav__item side-nav__item--active' : 'side-nav__item'
							}
						>
							<item.icon size={18} />
							<span>{item.label}</span>
						</NavLink>
					))}
				</nav>
				<div className="app__sidebar-foot">
					<ThemeToggle />
				</div>
			</aside>

			<header className="app__topbar">
				<NavLink to="/library" className="app__logo app__logo--sm">
					<img className="app__logo-mark" src="/icon.svg" alt="" />
					<span className="app__logo-text">Sumidoku</span>
				</NavLink>
				<ThemeToggle />
			</header>

			<main className="app__content">
				<Outlet />
			</main>

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
						<span className="tabbar__label">{item.label}</span>
					</NavLink>
				))}
			</nav>
		</div>
	);
}
