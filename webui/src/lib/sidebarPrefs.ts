const STORAGE_KEY = 'sumidoku.sidebar.collapsed';

export function readSidebarCollapsed(): boolean {
	try {
		return localStorage.getItem(STORAGE_KEY) === 'true';
	} catch {
		return false;
	}
}

export function writeSidebarCollapsed(collapsed: boolean) {
	try {
		localStorage.setItem(STORAGE_KEY, collapsed ? 'true' : 'false');
	} catch {
		// The toggle still works for this session when storage is unavailable.
	}
}
