/** Pure logic behind Select and the anchored menus, kept free of the DOM so it runs in tests */

export interface SelectOption {
	value: string;
	label: string;
}

/** Falls back to the first option, as a native select does when the value matches none */
export function selectedIndex(options: SelectOption[], value: string): number {
	const index = options.findIndex((option) => option.value === value);
	return index === -1 ? (options.length > 0 ? 0 : -1) : index;
}

/** The option a navigation key moves to, or null when the key does not navigate */
export function moveIndex(key: string, index: number, count: number): number | null {
	if (count === 0) return null;
	switch (key) {
		case 'ArrowDown':
			return Math.min(index + 1, count - 1);
		case 'ArrowUp':
			return Math.max(index - 1, 0);
		case 'Home':
			return 0;
		case 'End':
			return count - 1;
		default:
			return null;
	}
}

/** First label starting with the typed text, searching after `from` so repeating a letter cycles */
export function typeaheadIndex(options: SelectOption[], query: string, from: number): number {
	const needle = query.toLocaleLowerCase();
	// A repeated letter cycles through its matches instead of searching for "aa"
	const cycling = [...needle].every((char) => char === needle[0]);
	const prefix = cycling ? needle[0] : needle;
	const start = cycling ? from + 1 : from;
	for (let i = 0; i < options.length; i++) {
		const index = (start + i) % options.length;
		if (options[index].label.toLocaleLowerCase().startsWith(prefix)) return index;
	}
	return -1;
}

export interface MenuPlacement {
	top: number;
	left: number;
	maxHeight: number;
}

const GAP = 4;
const EDGE = 8;
const MAX_HEIGHT = 260;

/** Below the anchor unless only the space above fits; always clamped inside the viewport */
export function placeMenu(
	anchor: { top: number; bottom: number; left: number },
	menu: { width: number; height: number },
	viewport: { width: number; height: number }
): MenuPlacement {
	const below = viewport.height - anchor.bottom - GAP - EDGE;
	const above = anchor.top - GAP - EDGE;
	const up = menu.height > below && above > below;
	const maxHeight = Math.max(Math.min(up ? above : below, MAX_HEIGHT), 0);
	const height = Math.min(menu.height, maxHeight);
	return {
		top: up ? anchor.top - GAP - height : anchor.bottom + GAP,
		left: Math.max(EDGE, Math.min(anchor.left, viewport.width - menu.width - EDGE)),
		maxHeight
	};
}
