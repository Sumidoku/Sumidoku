import type { Mode } from './readerPrefs';

/**
 * Keyboard actions of the reader, mapped from physical keys in one pure function so
 * the mapping is testable without a DOM. `left`/`right` are physical sides (the
 * caller applies the reading direction); the rest are already reading-order
 * forward/back or scroll amounts. Escape stays with the component because closing
 * drawers and panels needs their state.
 */
export type ReaderKeyAction =
	| 'left'
	| 'right'
	| 'pageBack'
	| 'pageForward'
	| 'scrollBack'
	| 'scrollForward'
	| 'first'
	| 'last'
	| 'toggleOverlay'
	| 'toggleFullscreen'
	| 'cycleScale';

export interface KeyLike {
	key: string;
	shiftKey: boolean;
	ctrlKey: boolean;
	altKey: boolean;
	metaKey: boolean;
	target: EventTarget | null;
}

// Structural check (no HTMLElement instance) so the mapping is testable under SSR
export function isEditableTarget(target: EventTarget | null): boolean {
	const el = target as Partial<HTMLElement> | null;
	if (!el) return false;
	return (
		!!el.isContentEditable ||
		el.tagName === 'INPUT' ||
		el.tagName === 'TEXTAREA' ||
		el.getAttribute?.('role') === 'combobox'
	);
}

/**
 * Returns the reader action for a key event, or null when the key is not a reader
 * shortcut. Modifier chords are left to the browser/app, and keys aimed at form
 * controls (the settings sliders, the page slider) never reach the reader — the
 * control's own binding wins.
 */
export function readerKeyAction(e: KeyLike, mode: Mode): ReaderKeyAction | null {
	if (e.ctrlKey || e.altKey || e.metaKey) return null;
	if (isEditableTarget(e.target)) return null;
	switch (e.key) {
		case 'ArrowLeft':
			return mode === 'paged' ? 'left' : null;
		case 'ArrowRight':
			return mode === 'paged' ? 'right' : null;
		case 'ArrowUp':
			return mode === 'paged' ? null : 'scrollBack';
		case 'ArrowDown':
			return mode === 'paged' ? null : 'scrollForward';
		case 'PageUp':
			return mode === 'paged' ? 'pageBack' : 'scrollBack';
		case 'PageDown':
			return mode === 'paged' ? 'pageForward' : 'scrollForward';
		case 'Home':
			return 'first';
		case 'End':
			return 'last';
		case ' ':
			if (mode === 'paged') return e.shiftKey ? 'pageBack' : 'pageForward';
			return e.shiftKey ? 'scrollBack' : 'scrollForward';
		case 'm':
			return 'toggleOverlay';
		case 'f':
			return 'toggleFullscreen';
		case 'c':
			// Scale cycling only makes sense in paged mode; the scroll modes always fit width
			return mode === 'paged' ? 'cycleScale' : null;
		default:
			return null;
	}
}
