// Language filter of the repo install panel, stored in localStorage, kept across tabs and navigation

const LANGS_KEY = 'sumidoku.repoPrefs.langs';

export function loadRepoLangs(): string[] {
	try {
		const raw = localStorage.getItem(LANGS_KEY);
		if (raw === null) return [];
		const parsed: unknown = JSON.parse(raw);
		return Array.isArray(parsed) ? parsed.filter((c): c is string => typeof c === 'string') : [];
	} catch {
		// Private mode or broken JSON both fall back to no selection, silently
		return [];
	}
}

export function saveRepoLangs(langs: string[]): void {
	try {
		localStorage.setItem(LANGS_KEY, JSON.stringify(langs));
	} catch {
		// Private mode blocks the write; degrade silently (settings apply for this session only)
	}
}
