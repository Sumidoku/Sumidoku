export interface ReaderChrome {
	overlay: boolean;
	settingsOpen: boolean;
	shortcutsOpen: boolean;
	/** The bar is still showing from entry and will hide on its own unless the reader touches the chrome first */
	autoHidePending: boolean;
}

export type ReaderChromeAction =
	| 'toggleOverlay'
	| 'toggleSettings'
	| 'closeSettings'
	| 'toggleShortcuts'
	| 'closeShortcuts'
	| 'autoHide'
	| 'keepOverlay';

/** How long the bar stays up after the reader opens before it gets out of the way */
export const AUTO_HIDE_MS = 2000;

export const INITIAL_READER_CHROME: ReaderChrome = { overlay: true, settingsOpen: false, shortcutsOpen: false, autoHidePending: true };

/**
 * The control bar and its settings menu change together here, so no route that hides
 * the bar (tap zone, keyboard, zoomed tap) can leave the menu open for the next time
 * it shows. The hidden settings button can still take keyboard focus, so it does
 * nothing until the bar is back.
 *
 * The bar opens visible so a new reader sees where the controls are, then hides itself
 * once (autoHide). Any chrome action before that is the reader taking over, so it
 * cancels the pending hide rather than having the bar vanish under them.
 */
export function readerChromeReducer(state: ReaderChrome, action: ReaderChromeAction): ReaderChrome {
	switch (action) {
		case 'toggleOverlay':
			return state.overlay
				? { overlay: false, settingsOpen: false, shortcutsOpen: false, autoHidePending: false }
				: { ...state, overlay: true, autoHidePending: false };
		case 'toggleSettings':
			return state.overlay ? { ...state, settingsOpen: !state.settingsOpen, shortcutsOpen: false, autoHidePending: false } : state;
		case 'closeSettings':
			return { ...state, settingsOpen: false, autoHidePending: false };
		case 'toggleShortcuts':
			return { ...state, settingsOpen: false, shortcutsOpen: !state.shortcutsOpen, autoHidePending: false };
		case 'closeShortcuts':
			return { ...state, shortcutsOpen: false, autoHidePending: false };
		case 'autoHide':
			if (!state.autoHidePending) return state;
			if (!state.overlay || state.settingsOpen || state.shortcutsOpen) return { ...state, autoHidePending: false };
			return { ...state, overlay: false, autoHidePending: false };
		case 'keepOverlay':
			return state.autoHidePending ? { ...state, autoHidePending: false } : state;
	}
}
