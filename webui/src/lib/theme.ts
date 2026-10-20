import { browser } from '$app/env';
import { derived, writable } from 'svelte/store';

export type Theme = 'dark' | 'light' | 'system';
export type ResolvedTheme = 'dark' | 'light';

const STORAGE_KEY = 'sumidoku.theme';
const media = browser ? window.matchMedia('(prefers-color-scheme: dark)') : null;

function readStored(): Theme {
	if (!browser) return 'dark';
	try {
		const v = localStorage.getItem(STORAGE_KEY);
		return v === 'light' || v === 'system' ? v : 'dark';
	} catch {
		return 'dark';
	}
}

function resolve(t: Theme): ResolvedTheme {
	if (t === 'system') return media?.matches ? 'dark' : 'light';
	return t;
}

function apply(t: Theme) {
	if (browser) document.documentElement.dataset.theme = resolve(t);
}

/** 当前主题偏好（dark/light/system） */
export const theme = writable<Theme>(readStored());

/** 已解析的实际主题（system 落到 dark/light），用于图标等依赖具体明暗的展示 */
export const resolvedTheme = derived(theme, ($t) => resolve($t));

// index.html 的内联脚本已按 localStorage 落过一次，这里兜底纠偏（如存储被清后默认值不一致）
if (browser) {
	apply(readStored());
	media?.addEventListener('change', () => {
		theme.update((t) => {
			if (t === 'system') apply(t);
			return t;
		});
	});
}

export function setTheme(t: Theme) {
	theme.set(t);
	try {
		localStorage.setItem(STORAGE_KEY, t);
	} catch {
		// 隐私模式写不进去也照常切换，只是不持久化
	}
	apply(t);
}
