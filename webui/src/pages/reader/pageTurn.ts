export type PageTurnStyle = 'auto' | 'slide' | 'fade';

export const TURN_HOLD_MS = 500;

export function parsePageTurn(value: string | null): PageTurnStyle | null {
	return value === 'auto' || value === 'slide' || value === 'fade' ? value : null;
}

export function crossfadeEnabled(style: PageTurnStyle, landscape: boolean, animated: boolean, split: boolean): boolean {
	return animated && !split && (style === 'fade' || (style === 'auto' && landscape));
}

export function shouldRevealTurn(shownPage: number, targetPage: number, ready: boolean, split: boolean): boolean {
	return shownPage === targetPage || ready || split;
}
