export interface ReaderChrome {
	overlay: boolean;
	settingsOpen: boolean;
}

export type ReaderChromeAction = 'toggleOverlay' | 'toggleSettings' | 'closeSettings';

export const INITIAL_READER_CHROME: ReaderChrome = { overlay: true, settingsOpen: false };

/**
 * The control bar and its settings menu change together here, so no route that hides
 * the bar (tap zone, keyboard, zoomed tap) can leave the menu open for the next time
 * it shows. The hidden settings button can still take keyboard focus, so it does
 * nothing until the bar is back.
 */
export function readerChromeReducer(state: ReaderChrome, action: ReaderChromeAction): ReaderChrome {
	switch (action) {
		case 'toggleOverlay':
			return state.overlay ? { overlay: false, settingsOpen: false } : { ...state, overlay: true };
		case 'toggleSettings':
			return state.overlay ? { ...state, settingsOpen: !state.settingsOpen } : state;
		case 'closeSettings':
			return { ...state, settingsOpen: false };
	}
}
