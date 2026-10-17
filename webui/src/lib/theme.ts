import { useSyncExternalStore } from 'react';

export type Theme = 'dark' | 'light';

const STORAGE_KEY = 'sumidoku.theme';

function readStored(): Theme {
	try {
		return localStorage.getItem(STORAGE_KEY) === 'light' ? 'light' : 'dark';
	} catch {
		return 'dark';
	}
}

let current: Theme = readStored();
const listeners = new Set<() => void>();

// index.html 的内联脚本已按 localStorage 落过一次，这里兜底纠偏（如存储被清后默认值不一致）
document.documentElement.dataset.theme = current;

export function setTheme(t: Theme) {
	current = t;
	try {
		localStorage.setItem(STORAGE_KEY, t);
	} catch {
		// 隐私模式写不进去也照常切换，只是不持久化
	}
	document.documentElement.dataset.theme = t;
	listeners.forEach((l) => l());
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
