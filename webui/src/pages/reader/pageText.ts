import type { Page } from '../../api/types';

function isText(p: Page | undefined | null): p is Page & { content: { Text: string } } {
	return !!p?.content && typeof p.content === 'object' && 'Text' in p.content;
}

export function pageText(p: Page | undefined | null): string | null {
	if (!p) return null;
	if (p.hasDescription && typeof p.description === 'string') return p.description;
	if (isText(p)) return p.content.Text;
	return null;
}
