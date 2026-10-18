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

// index.html 的内联脚本已按 localStorage 落过一次，这里兜底纠偏（如存储被清后默认值不一致）
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
		// 隐私模式写不进去也照常切换，只是不持久化
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

/** 已解析的实际主题（system 落到 dark/light），用于图标等依赖具体明暗的展示 */
export function useResolvedTheme(): ResolvedTheme {
	return useSyncExternalStore(subscribe, () => resolve(current));
}
