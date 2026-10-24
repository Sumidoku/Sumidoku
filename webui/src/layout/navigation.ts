import { Clock, Compass, Download, Library, RefreshCw, Settings } from 'lucide-react';

export const NAV = [
	{ to: '/library', labelKey: 'common.nav.library', icon: Library },
	{ to: '/browse', labelKey: 'common.nav.browse', icon: Compass },
	{ to: '/history', labelKey: 'common.nav.history', icon: Clock },
	{ to: '/downloads', labelKey: 'common.nav.downloads', icon: Download },
	{ to: '/updates', labelKey: 'common.nav.updates', icon: RefreshCw },
	{ to: '/settings', labelKey: 'common.nav.settings', icon: Settings }
] as const;
