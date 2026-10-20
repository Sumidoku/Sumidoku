// 图标加载失败的色块底色：按 key hash 从 8 色板取，同源每次渲染颜色一致
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
