import { Clock, Compass, Download, Library, RefreshCw, Settings } from 'lucide-react';

export const NAV = [
	{ to: '/library', labelKey: 'common.nav.library', icon: Library },
	{ to: '/browse', labelKey: 'common.nav.browse', icon: Compass },
	{ to: '/history', labelKey: 'common.nav.history', icon: Clock },
	{ to: '/downloads', labelKey: 'common.nav.downloads', icon: Download },
	{ to: '/updates', labelKey: 'common.nav.updates', icon: RefreshCw },
	{ to: '/settings', labelKey: 'common.nav.settings', icon: Settings }
] as const;

export const MOBILE_PRIMARY_NAV = NAV.filter((item) => ['/library', '/browse', '/updates'].includes(item.to));
export const MOBILE_ACTIVITY_NAV = NAV.filter((item) => ['/history', '/downloads'].includes(item.to));
export const MOBILE_SETTINGS_NAV = NAV.filter((item) => item.to === '/settings');
