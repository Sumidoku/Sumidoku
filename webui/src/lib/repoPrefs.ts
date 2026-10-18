// 源仓库安装面板的语言过滤，存 localStorage，跨标签页与导航保留

const LANGS_KEY = 'sumidoku.repoPrefs.langs';

export function loadRepoLangs(): string[] {
	try {
		const raw = localStorage.getItem(LANGS_KEY);
		if (raw === null) return [];
		const parsed: unknown = JSON.parse(raw);
		return Array.isArray(parsed) ? parsed.filter((c): c is string => typeof c === 'string') : [];
	} catch {
		// 隐私模式读不到或坏 JSON 都回退空选，静默
		return [];
	}
}

export function saveRepoLangs(langs: string[]): void {
	try {
		localStorage.setItem(LANGS_KEY, JSON.stringify(langs));
	} catch {
		// 隐私模式写不进存储，静默降级（设置仅本轮生效）
	}
}
