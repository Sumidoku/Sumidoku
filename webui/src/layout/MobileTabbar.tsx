import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { NavLink, useLocation } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { Activity } from 'lucide-react';
import { MOBILE_ACTIVITY_NAV, MOBILE_PRIMARY_NAV, MOBILE_SETTINGS_NAV, NAV } from './navigation';

type NavItem = (typeof NAV)[number];

function MobileNavLink({ item }: { item: NavItem }) {
	const { t } = useTranslation();
	return (
		<NavLink
			to={item.to}
			className={({ isActive }) => isActive ? 'tabbar__item tabbar__item--active' : 'tabbar__item'}
		>
			<span className="tabbar__icon">
				<item.icon size={20} aria-hidden="true" />
			</span>
			<span className="tabbar__label">{t(item.labelKey)}</span>
		</NavLink>
	);
}

export function MobileTabbar() {
	const { t } = useTranslation();
	const { pathname } = useLocation();
	const [activityOpen, setActivityOpen] = useState(false);
	const rootRef = useRef<HTMLElement>(null);
	const activityButtonRef = useRef<HTMLButtonElement>(null);
	const activityMenuRef = useRef<HTMLDivElement>(null);
	const activityActive = MOBILE_ACTIVITY_NAV.some((item) => pathname.startsWith(item.to));

	useEffect(() => {
		setActivityOpen(false);
	}, [pathname]);

	useEffect(() => {
		if (activityOpen) activityMenuRef.current?.querySelector<HTMLAnchorElement>('a')?.focus();
	}, [activityOpen]);

	useEffect(() => {
		if (!activityOpen) return;
		const onPointerDown = (event: PointerEvent) => {
			if (!rootRef.current?.contains(event.target as Node)) setActivityOpen(false);
		};
		const onKeyDown = (event: globalThis.KeyboardEvent) => {
			if (event.key !== 'Escape') return;
			setActivityOpen(false);
			activityButtonRef.current?.focus();
		};
		document.addEventListener('pointerdown', onPointerDown);
		document.addEventListener('keydown', onKeyDown);
		return () => {
			document.removeEventListener('pointerdown', onPointerDown);
			document.removeEventListener('keydown', onKeyDown);
		};
	}, [activityOpen]);

	function onActivityMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
		if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
		event.preventDefault();
		const links = Array.from(activityMenuRef.current?.querySelectorAll<HTMLAnchorElement>('a') ?? []);
		if (links.length === 0) return;
		const current = links.indexOf(document.activeElement as HTMLAnchorElement);
		const next = event.key === 'Home'
			? 0
			: event.key === 'End'
				? links.length - 1
				: event.key === 'ArrowDown'
					? (current + 1 + links.length) % links.length
					: (current - 1 + links.length) % links.length;
		links[next]?.focus();
	}

	return (
		<nav className="app__tabbar" aria-label={t('layout.primaryNav')} ref={rootRef}>
			{MOBILE_PRIMARY_NAV.map((item) => <MobileNavLink key={item.to} item={item} />)}
			<div className="tabbar__activity">
				<button
					type="button"
					className={activityActive ? 'tabbar__item tabbar__item--active' : 'tabbar__item'}
					aria-current={activityActive ? 'page' : undefined}
					aria-expanded={activityOpen}
					aria-controls="mobile-activity-menu"
					aria-haspopup="menu"
					ref={activityButtonRef}
					onClick={() => setActivityOpen((open) => !open)}
				>
					<span className="tabbar__icon">
						<Activity size={20} aria-hidden="true" />
					</span>
					<span className="tabbar__label">{t('common.nav.activity')}</span>
				</button>
				{activityOpen && (
					<div
						id="mobile-activity-menu"
						className="tabbar__activity-menu"
						role="menu"
						aria-label={t('common.nav.activity')}
						ref={activityMenuRef}
						onKeyDown={onActivityMenuKeyDown}
					>
						{MOBILE_ACTIVITY_NAV.map((item) => (
							<NavLink
								key={item.to}
								to={item.to}
								role="menuitem"
								className={({ isActive }) => isActive ? 'tabbar__activity-item tabbar__activity-item--active' : 'tabbar__activity-item'}
								onClick={() => setActivityOpen(false)}
							>
								<item.icon size={18} aria-hidden="true" />
								<span>{t(item.labelKey)}</span>
							</NavLink>
						))}
					</div>
				)}
			</div>
			{MOBILE_SETTINGS_NAV.map((item) => <MobileNavLink key={item.to} item={item} />)}
		</nav>
	);
}
