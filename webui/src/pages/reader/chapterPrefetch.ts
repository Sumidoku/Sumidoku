import type { Page } from '../../api/types';
import { pageText } from './pageText';

/** How many of the next chapter's leading pages to probe when nearing the end of the current one */
export const PREFETCH_PAGE_COUNT = 4;

/** The reader treats a chapter as nearly done once this many pages remain (measured in spreads) */
export const PREFETCH_REMAINING = 4;

/** The server 501s the Image variant outright, so it is not probeable */
function isUnsupported(p: Page): boolean {
	return !!p.content && typeof p.content === 'object' && 'Image' in p.content;
}

/** The next chapter's first probeable pages: images only, in reading order, capped */
export function pickProbePages(pages: Page[] | null | undefined, count: number = PREFETCH_PAGE_COUNT): Page[] {
	if (!pages) return [];
	return pages.filter((p) => pageText(p) === null && !isUnsupported(p)).slice(0, count);
}
