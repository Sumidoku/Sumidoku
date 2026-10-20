import { useSyncExternalStore } from 'react';

export type Theme = 'dark' | 'light' | 'system';
export type ResolvedTheme = 'dark' | 'light';

const STORAGE_KEY = 'sumidoku.theme';
const media = window.matchMedia('(prefers-color-scheme: dark)');

function readStored(): Theme {
	try {
		const v = localStorage.getItem(STORAGE_KEY);
		return v === 'light' || v === 'system' ? v : 'dark';
	} catch {
		return 'dark';
	}
}

function resolve(t: Theme): ResolvedTheme {
	if (t === 'system') return media.matches ? 'dark' : 'light';
	return t;
}

let current: Theme = readStored();
const listeners = new Set<() => void>();

function apply() {
	document.documentElement.dataset.theme = resolve(current);
}

function notify() {
	listeners.forEach((l) => l());
}

// index.html's inline script has already applied the theme from localStorage once; this is a fallback correction (e.g. defaults disagree after storage was cleared)
apply();

media.addEventListener('change', () => {
	if (current === 'system') {
		apply();
		notify();
	}
});

export function setTheme(t: Theme) {
	current = t;
	try {
		localStorage.setItem(STORAGE_KEY, t);
	} catch {
		// In private mode the write fails but the theme still switches, just without persisting
	}
	apply();
	notify();
}

function subscribe(l: () => void) {
	listeners.add(l);
	return () => {
		listeners.delete(l);
	};
}

export function useTheme(): [Theme, (t: Theme) => void] {
	const theme = useSyncExternalStore(subscribe, () => current);
	return [theme, setTheme];
}

/** The resolved effective theme (system falls to dark/light), for UI that depends on actual brightness such as icons */
export function useResolvedTheme(): ResolvedTheme {
	return useSyncExternalStore(subscribe, () => resolve(current));
}
