// Fallback swatch color when an icon fails to load: hashed from the key into an 8-color palette, so the same source renders the same color every time
const ICON_COLORS = [
	'#7c6cf0',
	'#4f8ef7',
	'#3fb68b',
	'#d9a03f',
	'#e5534b',
	'#c86dd7',
	'#5fb8a6',
	'#8b9cf9'
];

export function iconColor(key: string): string {
	let h = 0;
	for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) | 0;
	return ICON_COLORS[Math.abs(h) % ICON_COLORS.length];
}
