export function isShortcutHelpKey(e: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean }): boolean {
	return e.key === 'h' && !e.ctrlKey && !e.altKey && !e.metaKey;
}

export interface ReaderChrome {
	overlay: boolean;
	settingsOpen: boolean;
	shortcutsOpen: boolean;
}

export type ReaderChromeAction = 'toggleOverlay' | 'toggleSettings' | 'closeSettings' | 'toggleShortcuts' | 'closeShortcuts';

export const INITIAL_READER_CHROME: ReaderChrome = { overlay: true, settingsOpen: false, shortcutsOpen: false };

/**
 * The control bar and its settings menu change together here, so no route that hides
 * the bar (tap zone, keyboard, zoomed tap) can leave the menu open for the next time
 * it shows. The hidden settings button can still take keyboard focus, so it does
 * nothing until the bar is back.
 */
export function readerChromeReducer(state: ReaderChrome, action: ReaderChromeAction): ReaderChrome {
	switch (action) {
		case 'toggleOverlay':
			return state.overlay ? { overlay: false, settingsOpen: false, shortcutsOpen: false } : { ...state, overlay: true };
		case 'toggleSettings':
			return state.overlay ? { ...state, settingsOpen: !state.settingsOpen, shortcutsOpen: false } : state;
		case 'closeSettings':
			return { ...state, settingsOpen: false };
		case 'toggleShortcuts':
			return { ...state, settingsOpen: false, shortcutsOpen: !state.shortcutsOpen };
		case 'closeShortcuts':
			return { ...state, shortcutsOpen: false };
	}
}
